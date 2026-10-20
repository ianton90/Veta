# Changelog

Versions follow [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.
Each build also carries a build number (shown in About and `--version`).
While the major version is 0, minor versions may change behaviour.

## 0.5.0 — first release

0.1.0 to 0.4.0 were never released; their work ships in 0.5.0. 0.4.0
marked the switch to the mimalloc allocator, which made large operations
several times faster on Windows; 0.5.0 marks full passes and saves that
use all cores (on a 10 GB file, faster than polars at sort, formula, edit,
CSV import and export).

**Desktop app (`veta`)**
- Open several Parquet files as tabs, including files larger than memory
  (paged reading). Opening a file while Veta runs adds it to the open
  window. New, blank files.
- Grid with in-place editing, row and column insert/delete/rename/move
  (the Rows tab inserts, deletes, keeps or removes any number of rows at
  the top, bottom or a row number),
  undo/redo, and Save/Save As that keep the file's writer settings.
- Writer settings (compression, encoding, dictionary, statistics, bloom
  filters, row groups) and file metadata editors.
- Transformations recorded as applied steps: change type (with an error
  column for failed values), filter, sort (including custom and external
  sort), replace, fill, trim and case, split and merge columns, computed
  columns, keep/remove rows and columns. Steps can be saved and loaded as
  JSON recipes.
- Column statistics pane (min and max only for numbers, dates, times and
  durations).
- The steps pane lists the cells an edit-cells step changed (its arrow);
  a value that doesn't fit its column makes the cell flash red.
- CSV and JSON import and export, with delimiter, quote, decimal and
  encoding options; values that don't fit a column's type are kept in an
  error column instead of being dropped.
- Nested types: structs, lists and maps, nested inside each other to any
  depth. The Nested tab flattens (Struct → Columns, List → Rows, All
  Levels → Columns) and nests (Columns → Struct, Rows → Lists by group);
  each command's tooltip explains it with a before/after example. New columns
  and struct fields of any nested type (lists, fixed-size lists, structs,
  maps), built level by level.
  Struct columns open in the grid (header arrow) into editable field
  columns. Opening a row shows the fields, items and entries of its nested
  cells; each struct, list or map inside opens in turn (its arrow or
  Space), indented. Values edit in place at any depth; list items are added
  at the end or inserted and deleted anywhere; map entries edit their keys
  (any key type) and values, with entries added and deleted.
  Add or remove struct fields at any depth (also inside list items and
  map values); insert list items and map entries at any position.
- Ribbon, light and dark themes (TOML), settings.

**Command line (`veta-cli`)**
- `info`, `schema`, `head`, `stats`, cell/row/column edits (rows at the
  top, bottom or by number; `keep-rows`), metadata and
  writer settings, `convert` between Parquet, CSV and JSON, and `apply` to
  run a recipe.

**Packages**
- Windows installer (per-user, no admin) and portable zip; Linux AppImage
  and tarball; macOS `.dmg` when built on a Mac. App icon.
- `.parquet` files open in Veta: set up by the Windows installer and by
  `Veta.AppImage --integrate` on Linux. On macOS, opening from Finder needs
  #84.

**Performance**
- Large files are read, filtered, sorted, converted and saved on all
  cores, with memory bounded; a last filter or sort writes straight into
  the saved file. `scripts/bench/compare.py` compares `veta-cli` with
  pandas and polars (`--profile` shows where Veta spends its time).
- Sorts split rows by key range and sort the ranges on all cores.
- Full passes read several pieces of the file at once, and saves encode
  the column chunks of several row groups at once.
- A memory budget (half of RAM by default; Settings, `--memory`).
- The mimalloc allocator: on Windows the system allocator made large
  sorts and saves several times slower.

**Known limits**
- macOS packages must be built on a Mac and are unsigned.
