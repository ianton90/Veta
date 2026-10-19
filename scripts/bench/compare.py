#!/usr/bin/env python3
"""Compares veta-cli with pandas and polars on the same operations (#105).

Each operation runs in its own process; the script records the wall time and
the peak memory of that process. The input file is read once before each run,
so every tool starts with it in the operating system's file cache, and the
order of the tools rotates from one operation to the next. Veta is measured
through veta-cli: the desktop app runs the same veta-core code in the
background and only shows a few rows.

Needs Python 3.10+ with pandas, polars, pyarrow and psutil:

    pip install pandas polars pyarrow psutil

Make a test file (plain columns, no nested ones), then run the comparison:

    python scripts/bench/compare.py generate bench.parquet --size-gb 2
    cargo build --release -p veta-cli
    python scripts/bench/compare.py run bench.parquet \\
        --veta target/release/veta-cli --out bench-results.md

`run --only sort,filter` runs some operations. Veta and polars run by
default; `--tools veta,polars,pandas` adds pandas (slow, and it loads the
whole file into memory). Outputs are written next to the input file and deleted after each
step (the first CSV export is kept for the CSV import, which every tool
runs on that same file).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

OPERATIONS = ["head", "stats", "filter", "sort", "formula", "edit", "to_csv", "from_csv"]
# pandas only on request (`--tools veta,polars,pandas`): it loads the whole
# file into memory and takes many minutes on large files.
TOOLS = ["veta", "polars"]
ALL_TOOLS = ["veta", "polars", "pandas"]

FILTER = ("amount", 500.0)  # rows with amount > 500 (about half)
SORT_BY = "amount"


# --- Generating the test file ------------------------------------------------


def generate(path: Path, size_gb: float, seed: int = 42) -> None:
    """Writes plain columns in row groups of 1M rows until the file reaches
    about `size_gb` GB."""
    import numpy as np
    import pyarrow as pa
    import pyarrow.parquet as pq

    rng = np.random.default_rng(seed)
    cities = np.array(["Lisbon", "Oslo", "Lima", "Kyoto", "Accra", "Quito", "Riga", "Perth"])
    categories = np.array([f"cat-{i:02}" for i in range(40)])
    target = size_gb * 1e9
    rows = 1_000_000
    start = 0
    schema = None
    writer = None
    try:
        while True:
            ids = np.arange(start, start + rows, dtype=np.int64)
            amount = rng.gamma(2.0, 250.0, rows).round(2)
            score = rng.normal(50, 15, rows)
            score_mask = rng.random(rows) < 0.05
            note_mask = rng.random(rows) < 0.3
            names = np.char.add("user-", rng.integers(0, 5_000_000, rows).astype(str))
            notes = np.char.add("note ", rng.integers(0, 1_000_000_000, rows).astype(str))
            table = pa.table(
                {
                    "id": ids,
                    "ts": pa.array(
                        1_700_000_000_000_000 + ids * 1_000_000 + rng.integers(0, 999_999, rows),
                        pa.timestamp("us"),
                    ),
                    "category": categories[rng.integers(0, len(categories), rows)],
                    "city": cities[rng.integers(0, len(cities), rows)],
                    "name": names,
                    "amount": amount,
                    "qty": rng.integers(1, 20, rows, dtype=np.int32),
                    "score": pa.array(score, mask=score_mask),
                    "flag": rng.random(rows) < 0.5,
                    "note": pa.array(notes, mask=note_mask),
                }
            )
            if writer is None:
                schema = table.schema
                writer = pq.ParquetWriter(path, schema, compression="snappy")
            writer.write_table(table, row_group_size=rows)
            start += rows
            if path.stat().st_size >= target:
                break
    finally:
        if writer is not None:
            writer.close()
    print(f"{path}: {start:,} rows, {path.stat().st_size / 1e9:.2f} GB")


# --- One operation, in a child process ---------------------------------------


def veta_command(veta: str, op: str, src: Path, out: Path, work: Path) -> list[str]:
    """The veta-cli command for `op`."""

    def recipe(steps: list[dict]) -> str:
        path = work / f"{op}.recipe.json"
        entries = [dict(order=i + 1, **s) for i, s in enumerate(steps)]
        path.write_text(json.dumps({"format": "veta-recipe", "version": 1, "steps": entries}))
        return str(path)

    column, value = FILTER
    match op:
        case "head":
            return [veta, "head", str(src), "-n", "10"]
        case "stats":
            return [veta, "stats", str(src), "--json"]
        case "filter":
            steps = [
                {
                    "type": "filter",
                    "settings": {
                        "match_all": True,
                        "conditions": [{"column": column, "op": "greater", "value": str(value)}],
                    },
                }
            ]
            return [veta, "apply", recipe(steps), str(src), "-o", str(out)]
        case "sort":
            steps = [{"type": "sort", "settings": {"keys": [{"column": SORT_BY}]}}]
            return [veta, "apply", recipe(steps), str(src), "-o", str(out)]
        case "formula":
            steps = [
                {
                    "type": "custom_column",
                    "settings": {"name": "total", "expression": "[amount] * [qty]", "at": 10},
                }
            ]
            return [veta, "apply", recipe(steps), str(src), "-o", str(out)]
        case "edit":
            return [
                veta, "set-cell", str(src), "--row", "1", "--column", "amount",
                "--value", "1.5", "-o", str(out),
            ]
        case "to_csv":
            return [veta, "convert", str(src), str(out)]
        case "from_csv":
            return [veta, "convert", str(src), str(out)]
    raise ValueError(op)


def python_op(tool: str, op: str, src: str, out: str) -> None:
    """Runs `op` with pandas or polars, written the way each is normally
    used for the job (polars lazily, streaming where it can)."""
    column, value = FILTER
    if tool == "polars":
        import polars as pl

        match op:
            case "head":
                print(pl.scan_parquet(src).head(10).collect())
            case "stats":
                print(pl.read_parquet(src).describe())
            case "filter":
                pl.scan_parquet(src).filter(pl.col(column) > value).sink_parquet(out)
            case "sort":
                pl.scan_parquet(src).sort(SORT_BY).sink_parquet(out)
            case "formula":
                lf = pl.scan_parquet(src)
                lf.with_columns((pl.col("amount") * pl.col("qty")).alias("total")).sink_parquet(out)
            case "edit":
                df = pl.read_parquet(src)
                df[0, "amount"] = 1.5
                df.write_parquet(out)
            case "to_csv":
                pl.scan_parquet(src).sink_csv(out)
            case "from_csv":
                pl.scan_csv(src, try_parse_dates=True).sink_parquet(out)
    else:
        import pandas as pd

        match op:
            case "head":
                print(pd.read_parquet(src).head(10))
            case "stats":
                print(pd.read_parquet(src).describe(include="all"))
            case "filter":
                df = pd.read_parquet(src)
                df[df[column] > value].to_parquet(out)
            case "sort":
                pd.read_parquet(src).sort_values(SORT_BY).to_parquet(out)
            case "formula":
                df = pd.read_parquet(src)
                df["total"] = df["amount"] * df["qty"]
                df.to_parquet(out)
            case "edit":
                df = pd.read_parquet(src)
                df.loc[df.index[0], "amount"] = 1.5
                df.to_parquet(out)
            case "to_csv":
                pd.read_parquet(src).to_csv(out, index=False)
            case "from_csv":
                pd.read_csv(src).to_parquet(out)


# --- Measuring ----------------------------------------------------------------


def warm(path: Path) -> None:
    """Reads `path` once so it is in the operating system's file cache: every
    tool then starts from the same warm cache, whatever ran before it (pandas
    can use enough memory to push the file out)."""
    with open(path, "rb") as f:
        while f.read(16 << 20):
            pass


def measure(command: list[str]) -> tuple[float, float, int]:
    """Runs `command`; returns wall seconds, peak memory in MB and the exit
    code. The peak is the operating system's count where it has one (Linux,
    macOS), else the resident size polled every 20 ms with psutil."""
    import psutil

    start = time.perf_counter()
    # UTF-8 output in child Pythons: polars prints tables with box-drawing
    # characters that Windows' default console encoding can't write.
    env = dict(os.environ, PYTHONUTF8="1")
    proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
    ps = psutil.Process(proc.pid)
    peak = 0
    code = None
    while code is None:
        try:
            rss = ps.memory_info().rss + sum(
                c.memory_info().rss for c in ps.children(recursive=True)
            )
            peak = max(peak, rss)
        except psutil.Error:
            pass
        if hasattr(os, "wait4"):
            pid, status, usage = os.wait4(proc.pid, os.WNOHANG)
            if pid:
                code = os.waitstatus_to_exitcode(status)
                # Kilobytes on Linux, bytes on macOS.
                scale = 1 if sys.platform == "darwin" else 1024
                peak = max(peak, usage.ru_maxrss * scale)
                proc.returncode = code
                break
        else:
            code = proc.poll()
        time.sleep(0.02)
    seconds = time.perf_counter() - start
    if code != 0:
        sys.stderr.write(proc.stderr.read().decode(errors="replace")[-2000:])
    return seconds, peak / 1e6, code


def run(args: argparse.Namespace) -> None:
    src = Path(args.file).resolve()
    ops = args.only.split(",") if args.only else OPERATIONS
    tools = args.tools.split(",") if args.tools else TOOLS
    work = Path(tempfile.mkdtemp(prefix="veta-bench-", dir=src.parent))
    results: dict[str, dict[str, tuple[float, float, int]]] = {}
    csv_inputs: dict[str, Path] = {}
    try:
        for n, op in enumerate(ops):
            results[op] = {}
            # Rotate the order so no tool always runs first or last.
            order = tools[n % len(tools):] + tools[: n % len(tools)]
            for tool in order:
                ext = ".csv" if op == "to_csv" else ".parquet"
                out = work / f"{op}-{tool}{ext}"
                input_file = src
                if op == "from_csv":
                    # Every tool imports the same CSV: the first export.
                    input_file = next(iter(csv_inputs.values()), None)
                    if input_file is None:
                        print("from_csv needs to_csv to run first; skipped")
                        break
                if tool == "veta":
                    command = veta_command(args.veta, op, input_file, out, work)
                else:
                    command = [sys.executable, __file__, "op", tool, op, str(input_file), str(out)]
                warm(Path(input_file))
                seconds, mb, code = measure(command)
                results[op][tool] = (seconds, mb, code)
                status = "" if code == 0 else f"  (failed, exit {code})"
                print(f"{op:9} {tool:7} {seconds:8.2f} s {mb:9.0f} MB{status}", flush=True)
                if op == "to_csv" and code == 0 and not csv_inputs:
                    csv_inputs[tool] = out
                elif out.exists():
                    out.unlink()
            if op == "from_csv":
                for path in csv_inputs.values():
                    path.unlink(missing_ok=True)
                csv_inputs.clear()
    finally:
        shutil.rmtree(work, ignore_errors=True)
    report = format_report(src, results, tools)
    print()
    print(report)
    if args.out:
        Path(args.out).write_text(report, encoding="utf-8")


def format_report(src: Path, results: dict, tools: list[str]) -> str:
    size = src.stat().st_size / 1e9
    lines = [
        f"File: {src.name}, {size:.2f} GB. Time in seconds, peak memory in MB.",
        "",
        "| Operation | " + " | ".join(f"{t} s | {t} MB" for t in tools)
        + (" | veta / polars time |" if "veta" in tools and "polars" in tools else " |"),
        "|---|" + "---:|---:|" * len(tools) + ("---:|" if "veta" in tools and "polars" in tools else ""),
    ]
    for op, by_tool in results.items():
        cells = []
        for tool in tools:
            seconds, mb, code = by_tool.get(tool, (0.0, 0.0, -1))
            cells.append("failed | failed" if code != 0 else f"{seconds:.2f} | {mb:.0f}")
        row = f"| {op} | " + " | ".join(cells)
        if "veta" in tools and "polars" in tools:
            veta, polars = by_tool.get("veta"), by_tool.get("polars")
            ratio = (
                f"{veta[0] / polars[0]:.2f}×"
                if veta and polars and veta[2] == 0 and polars[2] == 0 and polars[0] > 0
                else "–"
            )
            row += f" | {ratio}"
        lines.append(row + " |")
    return "\n".join(lines) + "\n"


def main() -> None:
    # The report uses × and –, which Windows' default console encoding lacks.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    g = sub.add_parser("generate", help="write a test file")
    g.add_argument("file")
    g.add_argument("--size-gb", type=float, default=2.0)
    r = sub.add_parser("run", help="compare the tools on a file")
    r.add_argument("file")
    r.add_argument("--veta", default="target/release/veta-cli", help="path to veta-cli")
    r.add_argument("--only", help=f"comma-separated operations ({','.join(OPERATIONS)})")
    r.add_argument(
        "--tools",
        help=f"comma-separated tools ({','.join(ALL_TOOLS)}; default {','.join(TOOLS)})",
    )
    r.add_argument("--out", help="also write the results table (Markdown) here")
    o = sub.add_parser("op", help=argparse.SUPPRESS)
    o.add_argument("tool")
    o.add_argument("op")
    o.add_argument("src")
    o.add_argument("out")
    args = parser.parse_args()
    if args.command == "generate":
        generate(Path(args.file), args.size_gb)
    elif args.command == "run":
        run(args)
    else:
        python_op(args.tool, args.op, args.src, args.out)


if __name__ == "__main__":
    main()
