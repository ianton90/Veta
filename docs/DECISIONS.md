# Decisions

Short log of project decisions. Newest last.

| # | Date | Decision | Why |
|---|------|----------|-----|
| 1 | 2026-10-09 | Rust, Cargo workspace (`veta-core`, `veta-gui`, `veta-cli`, `veta`) | Separate UI-free core shared by GUI and CLI. |
| 2 | 2026-10-09 | GUI with `iced` (latest stable) | Owner's choice; pure Rust, cross-platform. |
| 3 | 2026-10-09 | `arrow` + `parquet` instead of `polars` | Full control over writer settings, encodings and metadata; far fewer dependencies and faster builds. Cost: larger-than-memory operations (e.g. sort) are ours to write. |
| 4 | 2026-10-09 | `clap` for the CLI | De facto standard; generates help and validates arguments. |
| 5 | 2026-10-09 | `rfd` for file dialogs | iced has no native file dialogs. |
| 6 | 2026-10-09 | `serde` + `toml` for themes, config and recipes | `serde` maps TOML text onto Rust structs automatically instead of hand-written parsing. |
| 7 | 2026-10-09 | MVC: all data changes go through `Command`s in `veta-core` | One code path for GUI, CLI and undo/redo. |
| 8 | 2026-10-09 | Data changes are Power Query-style applied steps | Inspectable, reorderable, replayable from the CLI. |
| 9 | 2026-10-09 | Files above a memory budget are paged by row group | Must handle files larger than RAM. |
| 10 | 2026-10-09 | Save preserves the source file's writer settings | Owner requirement; defaults only for new files. |
| 11 | 2026-10-09 | Apache-2.0 license | Free to fork and use; attribution required. |
| 12 | 2026-10-09 | Branches: `dv` development, `pr` release. No CI. | Owner's setup; checks run locally before pushing. |
| 13 | 2026-10-09 | Enable arrow's `chrono-tz` feature | Needed to display timestamps with named time zones (e.g. `UTC`, `Europe/Madrid`). |
| 14 | 2026-10-09 | Files are read in chunks of 64k rows within row groups, cached LRU under the memory budget (default 1 GiB) | Memory stays bounded even when a single row group is larger than the budget. |
| 15 | 2026-10-09 | Grid is a custom iced widget drawing only visible cells | iced has no table widget; a custom widget keeps scrolling smooth on millions of rows. |
| 16 | 2026-10-09 | Icons: a subset of the Lucide icon font vendored in `crates/veta-gui/assets` (ISC license) | No extra crate (an SVG renderer would pull in many); 18 KB. |
| 17 | 2026-10-09 | Themes are TOML files; two built in, users add more in `<config dir>/themes` | Owner requirement; easy to share and edit. |
| 18 | 2026-10-09 | Settings in `<config dir>/config.toml`; config dir resolved by hand (`%APPDATA%`, `~/Library/Application Support`, `$XDG_CONFIG_HOME`), overridable with `VETA_CONFIG_DIR` | Avoids a dependency for three paths. |
| 19 | 2026-10-09 | Font family and size apply on restart; colors and light/dark switch live | iced sets the default font at startup. |
| 20 | 2026-10-10 | Cell edits and row insert/delete refer to rows by position at their step. Changing an earlier step warns the user instead of tracking row identity. | Owner's choice: simpler, and stable IDs cause surprises after sorting. |
| 21 | 2026-10-10 | Consecutive cell edits stay grouped in one step; each edit undoes on its own. | Keeps the steps list readable. |
| 22 | 2026-10-10 | Steps that need a full pass (filter, sort, fill) cache their result: filters keep the surviving row ranges; sort/fill materialize their output in memory for in-memory files, or to a temporary Parquet file for paged files. Computed in the background; changing a step drops the caches from that step on. | Full results scrollable for files larger than RAM. |
| 23 | 2026-10-10 | Changing an earlier step may break later ones; they are marked with the error, not refused. Saving and adding steps are blocked until fixed. | Same as Power Query. |
| 24 | 2026-10-10 | Date/time formats in Cast use our own parser for a strftime subset (`%Y %y %m %d %b %B %H %I %p %M %S %f`) | `chrono` would need a direct dependency (arrow does not re-export it); about 150 lines instead. |
| 25 | 2026-10-10 | Computed columns use a small expression language parsed in core: `[Column]` or bare names, `+ - * / % &`, comparisons, `and/or/not`, a fixed set of functions. Duplicate and Index columns are computed columns (`[col]`, `row()`). | No dependency; type-checked against the schema so the column type is known without reading data. |
| 26 | 2026-10-10 | `veta-core` uses `serde` + `toml` for recipes, through its own recipe types (steps stay serde-free); types written as in `veta schema`, edited cell values as text | Recipes are shared by the GUI and CLI; keeps arrow types out of the file format. |
| 27 | 2026-10-11 | Row filtering is a temporary view (not a step), computed in the background; the table is read-only while it is on. Keep/Remove Rows turn it into a filter step (Remove = inverted). Right-click Keep/Delete rows act on the selection. | Owner: separate filtering from keeping, like Excel. |
| 28 | 2026-10-11 | Recipes are JSON via `serde_json` (replaces the TOML of 26): metadata plus `steps`, each with `order`, `type`, `description`, `settings`. `veta-core` no longer uses `toml`. | Owner: readable by people, not just parsers; `serde_json` approved. |
| 29 | 2026-10-11 | CSV/JSON import and export use arrow's `csv`/`json` readers and writers (part of the approved `arrow` crate). Imports read all CSV columns as text and convert them with the cast logic (invalid values become null; decimal comma supported); results go through a temporary Parquet file, loaded into memory if they fit the budget, paged otherwise. Encodings: UTF-8 and Latin-1 only. | No new crates; large files stream. Other encodings would need `encoding_rs`. |
