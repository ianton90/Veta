# Veta — Scope

Veta is a desktop application for reading and editing Apache Parquet files,
in the spirit of Microsoft Access / Power Query, with a companion command-line
interface that exposes the same editing capabilities.

## Goals

### Files
- Open Parquet files of any size. Files that fit in the memory budget are
  loaded entirely; larger files are paged (read on demand, row group by row
  group). The budget is configurable.
- Multiple open files, each in its own tab. Tabs sit at the bottom of the
  window, like Excel sheets.
- Save (in place) and Save As.
- On save, preserve the original writer settings: row-group size, page size,
  per-column compression and encoding, dictionary usage, statistics, format
  version. Defaults apply only to new files.
- A dialog to inspect and change writer settings.
- Import from and export to other formats: CSV and JSON first; others later.

### Editing
- Edit individual cells.
- Add, delete and reorder rows and columns.
- Edit metadata: file key/value metadata, column names, schema/types and
  writer settings.
- Undo/redo for every change, from v1.

### Transformations (applied steps)
- Every change to the data is recorded as a step in an ordered list, like
  Power Query's "Applied Steps": steps can be inspected, edited, removed and
  reordered.
- Changing a column's type (cast) is a transformation with explicit options
  (e.g. decimal separator `.` vs `,`, date formats, what to do on failure).
- Steps can be saved to a recipe file and replayed from the CLI.

### Analytics
- A side pane with per-column statistics: count, null count, distinct count,
  min, max, mean, standard deviation, to start. More will be added over time.

### Nested types
- Struct, list and map columns. Displayed read-only at first; editing later.

### User interface
- Ribbon menus (Power Query's ribbon as the starting point).
- Configurable themes: font, color scheme, light/dark mode. Themes are TOML
  files; a default light and dark theme ship with the app.

### CLI
- The same `veta` binary: with no arguments it opens the GUI; with a
  subcommand it runs headless (e.g. `veta info file.parquet`,
  `veta set-cell ...`, `veta apply recipe.toml ...`, `veta convert ...`).

### Platforms
- Windows, macOS, Linux. Mobile is out of scope.

## Non-goals (for now)
- Mobile platforms.
- Multi-file datasets / partitioned directories (may come later).
- Remote storage (S3, HTTP, etc.).
- SQL query engine.
- Formulas per cell like Excel; computations happen through steps.

## Stack and constraints
- Rust (stable), Cargo workspace.
- UI: [iced](https://iced.rs) (latest stable, currently 0.14).
- Minimal dependencies. Approved so far:
  - `arrow`, `parquet` — official Apache Arrow/Parquet crates (data, compute
    kernels, CSV/JSON readers and writers).
  - `iced` — GUI.
  - `clap` — CLI argument parsing.
  - `rfd` — native file dialogs.
  - `serde` + `toml` — config, themes and recipe files.
  - Any new dependency must be justified in `docs/DECISIONS.md`.
- Model–View–Controller architecture (see `ARCHITECTURE.md`).
- License: Apache-2.0.

## Process
- Work is tracked in GitHub issues: one parent issue per milestone, with
  sub-issues as tasks.
- Development happens on the `dv` branch with direct pushes. `pr` is the
  release branch.
- No CI: `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test`
  are run locally before every push.
