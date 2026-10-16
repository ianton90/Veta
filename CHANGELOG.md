# Changelog

Versions follow [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.
Each build also carries a build number (shown in About and `--version`).
While the major version is 0, minor versions may change behaviour.

## 0.2.0 — first release

0.1.0 was never released; its work ships in 0.2.0.

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
- Column statistics pane.
- CSV and JSON import and export, with delimiter, quote, decimal and
  encoding options; values that don't fit a column's type are kept in an
  error column instead of being dropped.
- Nested types: structs, lists and maps, nested inside each other to any
  depth; expand a struct into columns or explode a list into rows; combine
  columns into a struct or collect them into lists by group; new columns
  and struct fields of any nested type (lists, fixed-size lists, structs,
  maps), built level by level.
  Struct columns open in the grid (header arrow) into editable field
  columns. Opening a row shows the fields, items and entries of its nested
  cells; each struct, list or map inside opens in turn (its arrow or
  Space), indented. Values edit in place at any depth; list items are added
  at the end or inserted and deleted anywhere; map entries edit their keys
  (any key type) and values, with entries added and deleted.
  Add or remove fields of struct columns.
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

**Known limits**
- macOS packages must be built on a Mac and are unsigned.
