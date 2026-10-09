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
