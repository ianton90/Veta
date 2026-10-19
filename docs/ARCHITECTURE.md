# Veta — Architecture

## Overview

```
                ┌──────────────┐     ┌──────────────┐
   Views        │  veta-gui    │     │  veta-cli    │
                │  (iced)      │     │  (clap)      │
                └──────┬───────┘     └──────┬───────┘
                       │  Commands          │  Commands
                ┌──────▼────────────────────▼───────┐
   Controller   │  veta-core::controller             │
                │  validates + applies commands,     │
                │  records undo/redo history         │
                └──────┬─────────────────────────────┘
                ┌──────▼─────────────────────────────┐
   Model        │  veta-core::model                  │
                │  Document = Source + Steps +       │
                │  Metadata + WriterSettings         │
                └──────┬─────────────────────────────┘
                ┌──────▼─────────────────────────────┐
   Storage      │  veta-core::io                     │
                │  parquet read (in-memory / paged), │
                │  parquet write, CSV/JSON import/   │
                │  export                            │
                └────────────────────────────────────┘
```

## Workspace layout

```
crates/
  veta-core/   library: model, controller, io, steps, stats. No UI deps.
  veta-gui/    library: iced application (views + message → command mapping).
  veta-cli/    library: clap definitions + command runners.
  veta/        binary: no args → GUI, subcommand → CLI.
  veta-testkit/ dev-only: Parquet fixture generators, temp dirs.
```

`veta-core` never depends on `iced` or `clap`. Everything that the GUI can do
to a file is a `Command` in core, so the CLI gets it for free.

## File map

Folders group files by purpose, at most two levels below `src/`. A folder's
`mod.rs` says what it holds and re-exports what callers use; every file
starts with a `//!` comment saying what it is for.

```
veta-core/src/
  lib.rs                 module list; short paths (io, command, rows, …)
  error.rs
  parallel.rs            independent work on all cores
  memory.rs              the memory budget (half of RAM by default)
  model/                 Document, Workbook, metadata, undo history
  controller/            the only way to change a Document
    command.rs           the Command enum
    mod.rs               execute / undo / redo; plan() hands each command
                         to its area: cells, nested, rows, columns,
                         metadata, steps
  steps/                 Step, Pipeline (lazy evaluation), full passes
                         (compute), checkpoints, cell edits, row ranges
  transforms/            what steps do: cast, filter, sort, fill, text,
                         split, nested (expand, explode, collect, combine)
  formulas/              custom column expressions: parser, eval, functions
  stats/                 column statistics
  files/                 parquet (open, save), source (in memory / paged),
                         recipe (JSON), convert/ (CSV, JSON, encodings,
                         export)
  values/                display (cell text, types, paths), node (one
                         value and its children, at any depth), parse
                         (text to values, nested edits), clipboard
veta-core/tests/         integration tests: workflows, recipes, large
                         files, import/export, edge files

veta-gui/src/
  lib.rs                 module list; run()
  app/                   the iced application
    mod.rs               startup, update, the Message dispatcher
    state.rs             App and the types it holds (Tab, Dialog, Edit, …)
    messages.rs          Message
    actions.rs           the ribbon Action dispatcher; panes, theme
    view.rs              the window
    documents.rs editing.rs clipboard.rs rows.rs columns.rs steps.rs
    filter.rs stats.rs menus.rs
                         one file per area: its message handlers, then its
                         ribbon actions, then its helpers
    tests/               App tests by area
  grid/                  the virtualized table
    mod.rs               GridView: per-tab state, selection, events
    geometry.rs          columns on screen, row lines, hit testing
    items.rs             open cells: nested values as trees of lines
    load.rs              row windows, background reads, filter view
    input.rs             the Grid widget and its event handling
    draw.rs              drawing
  panes/                 formula bar, status bar, right-click menu,
                         side_pane/ (steps, stats, file tabs)
  dialogs/               one file per dialog; mod.rs has the shared
                         widgets and the StepDialog trait
  settings/              Settings dialog, config.toml, themes
  platform/              native file dialogs, single instance, icon font
  ribbon.rs              the ribbon and its Actions

veta-cli/src/
  lib.rs                 clap definitions, run(), execute()
  commands/              inspect, edit, meta, convert, apply; shared
                         helpers in mod.rs
  table.rs               text tables
veta-cli/tests/cli.rs    runs the real binary

veta/src/main.rs         the desktop binary
veta-testkit/            Parquet fixtures and temp dirs for tests
```

## Request flow

How a click becomes a change on screen, e.g. Rows → Remove Rows → OK:

1. **Message.** The dialog's OK button produces
   `Message::Rows(RowsMessage::Apply)`; iced calls `App::update`
   (`app/mod.rs`).
2. **Update.** `App::handle` dispatches by area to `rows_message`
   (`app/rows.rs`), which asks the dialog for its command:
   `Command::DeleteRows { rows }`.
3. **Command.** The handler passes it to `veta_core::controller::execute`
   (most handlers do so through `App::execute`, which also refreshes the
   grid and reports errors).
4. **Controller.** `plan()` hands it to `controller/rows.rs`, which checks it
   and returns a label and a `Change` (here, push a `Step::DeleteRows`).
   `execute` applies the change to the `Document` and records it in the
   undo history.
5. **Pipeline.** The document's `Pipeline` now has one more step. Nothing is
   computed yet; steps that need a full pass (filter, sort, fill) leave it
   `Computing` and the app runs them on a worker thread.
6. **View.** `App::refresh` tells the tab's `GridView` the data changed;
   the grid reads only the rows on screen (`grid/load.rs`), through the
   pipeline, and `App::view` (`app/view.rs`) draws the window.

Step dialogs (filter, sort, cast, …) take the same path through one
`Message::Step` handler (`app/steps.rs`) and `App::add_or_replace`.

## MVC mapping

iced uses the Elm architecture (`state`, `update(message)`, `view(state)`).
We map it onto MVC as follows:

- **Model** (`veta-core::model`): `Workbook` (open documents) and `Document`.
  Plain data, no I/O side effects beyond reading through `io`.
- **Controller** (`veta-core::controller`): the only way to change a
  `Document`. Takes a `Command`, validates it, applies it, pushes it onto the
  undo history. Returns an error or a change summary.
- **View**:
  - GUI: iced `view` functions render the model. iced `Message`s that change
    data are translated to `Command`s and sent to the controller; messages
    that only affect presentation (scroll position, selected tab, open
    ribbon tab, theme) live in a GUI-only `UiState`.
  - CLI: each subcommand builds one or more `Command`s, runs them through
    the controller and prints the result.

## Document model

```
Document
├── source: DataSource          // the original file (read-only)
├── steps: Vec<Step>            // applied steps, in order
├── metadata: FileMetadata      // key/value metadata, created_by, ...
├── writer: WriterSettings      // row group size, compression, encodings...
├── history: History            // undo/redo stacks of Commands
└── path: Option<PathBuf>       // None for new/imported documents
```

### DataSource
- `InMemory(Vec<RecordBatch>)` when the file fits the memory budget.
- `Paged { reader, cache }` otherwise: reads row groups on demand into an LRU
  cache of a quarter of the budget.
- Both binaries use `mimalloc` as their allocator (decision 46): with the
  Windows system heap, the multi-GB allocations of sorts and saves were
  several times slower. Only visible on Windows.
- The memory budget (`memory.rs`) defaults to half of RAM; Settings and the
  CLI's `--memory` change it.
- Paged sorts sort by key range (`transforms/sort/buckets.rs`): sampled
  keys cut the range into buckets of ~48 MB, rows go to their bucket as
  they are read, and buckets are sorted on all cores and come out in order
  (no merge). Buckets stay in memory when the input fits half the budget,
  compacted every 64 input batches (a slice of every input batch made a
  large sort far slower); otherwise they go to temporary part files
  whenever they hold a quarter of the budget.
- Both expose the same trait: schema, row count, `read(range) -> RecordBatch`.
- Full passes and saves (save, filter, stats, CSV/JSON export) read the
  output in pieces of 1M rows, several at once on all cores
  (`Pipeline::read_parallel`, results in order); the paged source keeps a
  reader per piece being read and shares the cores between them. The writer
  moves columns to other workers each row group, so row groups encode at
  the same time.

### Steps
Every data change is a `Step`, Power Query style:

- Cell edits: consecutive edits collapse into one `EditCells` step holding a
  sparse map `(row, column) → value`.
- Row/column changes: `InsertRows`, `DeleteRows`, `AddColumn`,
  `RemoveColumns`, `RenameColumn`, `ReorderColumns`.
- Transformations: `Cast { column, to, options }`, `Filter`, `Sort`,
  `ReplaceValues`, `SplitColumn`, computed columns, …

Each step transforms the output of the previous one. Steps are `serde`
serializable, so a list of steps is a recipe file the CLI can replay.

Evaluation is lazy and windowed: to display rows `a..b` we evaluate the steps
only for that window when every step is row-preserving (map-like). Steps that
change row order or count (filter, sort, delete rows) build and cache a row
index for their output. Sorting data larger than memory needs an external
sort; that is tracked as its own task.

### Undo/redo
`History` holds the `Command`s applied, each with enough data to revert it
(e.g. the previous value of an edited cell, the removed step). Undo pops and
reverts; redo re-applies. Merging consecutive cell edits into one step is
undone one edit at a time.

### Saving
- Data is produced by streaming the evaluated steps batch by batch into the
  parquet `ArrowWriter`, so large files never need to fit in memory.
- `WriterSettings` is read from the source file's metadata on open (row group
  size, per-column compression/encoding, dictionary, statistics, page size,
  format version) and reused on save. New documents get defaults.
- Save in place writes to a temp file in the same directory and atomically
  renames it over the original once complete (the original is still being
  read during the write).
  Steps and undo history survive the save: the document keeps reading its
  source data from the old file through the open handle (or from memory),
  so undoing past the save works. The old file's disk space is freed when
  the document is closed.

## GUI layout

```
┌──────────────────────────────────────────────────────────┐
│ Ribbon: [Home] [Rows] [Columns] [Nested] [View]           │
├────────────────────────────────────────┬─────────────────┤
│                                        │ Side pane:      │
│   Data grid (virtualized)              │ Applied steps / │
│                                        │ Statistics /    │
│                                        │ Metadata        │
├────────────────────────────────────────┴─────────────────┤
│ File tabs: [sales.parquet] [users.parquet] [+]            │
├──────────────────────────────────────────────────────────┤
│ Status bar: rows, columns, memory mode, selection         │
└──────────────────────────────────────────────────────────┘
```

- The grid is virtualized: only visible rows are requested from the model.
- iced has no built-in table or ribbon widgets; both are custom.
- Ribbon tabs: Home (file, recipes, clipboard, undo, everyday commands),
  Rows, Columns, Nested, View. All tabs are always shown; commands that
  don't apply to the selection are greyed out.
- Clipboard: Ctrl+C/X/V copy, cut and paste tab-separated text (as
  spreadsheets do) for a cell, rows, or whole columns (with their names).
  A paste is one undo step and adds rows if needed.
- Themes: TOML files with font family/size, palette and light/dark flag,
  converted to an iced `Theme` plus Veta's own style tokens.

## Single instance

A second `veta` launch hands its files to the running window and exits
(`veta-gui::platform::instance`). The first instance listens on a Unix domain socket,
`veta.sock`, in a per-user private directory: `$XDG_RUNTIME_DIR/veta` or the
config dir on Linux, the config dir on macOS, `%LOCALAPPDATA%\veta` on
Windows (`$VETA_INSTANCE_DIR` overrides it). On Unix the directory is mode
0700; on Windows it inherits the user-only ACL. The client sends `veta 1\n`
then NUL-terminated absolute paths; the server answers `ok`, opens them as
tabs and focuses the window. A socket left by a crash is replaced. If
anything fails, the launch simply opens its own window. Windows uses
`uds_windows` until `std::os::windows::net` is stable.

## CLI

The desktop app is `veta [FILE]...`. Headless commands are a separate
binary, `veta-cli` (crate `veta-cli`), so any number can run in parallel
and the app needs no console window:

```
veta-cli info | schema <file>
veta-cli head <file> [-n N]
veta-cli set-cell <file> --row R --column C (--value V | --null)
veta-cli insert-rows <file> (--top | --bottom | --at R) [--count N]
veta-cli remove-rows <file> (--top N | --bottom N | --rows 5,10-20)   # alias delete-rows
veta-cli keep-rows <file> (--top N | --bottom N | --rows 5,10-20)
veta-cli add-column <file> <name> --type T [--at P]
veta-cli remove-column <file> <name>...
veta-cli rename-column <file> <from> <to>
veta-cli move-column <file> <name> --to P
veta-cli meta list|get|set|remove <file> [key [value]] [-c COLUMN]
veta-cli writer-settings <file> [--column C]... [--compression ...] [--level N]
          [--encoding ...] [--dictionary on|off] [--statistics none|chunk|page]
          [--bloom-filter on|off] [--row-group-rows N] [--format-version 1|2]
veta-cli stats <file> [-c NAME]... [--json]
veta-cli convert <in> <out> [--from F] [--format parquet|csv|json|ndjson]
          [--delimiter C] [--quote C|none] [--decimal C] [--no-header]
          [--encoding utf8|latin1|windows1252|utf16le|utf16be] [--date-format F]
          [--on-error column|null|fail]
veta-cli apply <recipe.json> <file>        # run steps saved from the GUI
```

Rows and positions are numbered from 1, as in the GUI. Commands that modify
write in place unless `-o FILE` is given. 
Recipes (`veta_core::recipe`) are JSON: metadata (`format`, `version`, `name`,
`description`, `created_with`, `source_file`) and a `steps` array; each step
has `order`, `type`, a readable `description` (ignored on load) and
`settings`.

## Testing

- Unit tests sit next to the code: inline in a `#[cfg(test)] mod tests` when
  short, in a sibling `tests.rs` (or `tests/` folder) when over about 150
  lines.
- `veta-core/tests/`: integration tests through the public API, on
  fixtures generated in the tests (no binary fixtures committed).
- `veta-cli/tests/cli.rs`: runs the real `veta-cli` binary.
- `veta-gui`: tests drive `App::update` with messages, as the window would;
  visual checks are screenshots under Xvfb (see `CLAUDE.md`).

## Step status

Steps are evaluated in order until one fails or needs a full pass over its
input (filter, sort, fill). The pipeline's status is `Ready`, `Computing
{ step }` or `Broken { step, error }`; the document shows the output of the
last evaluated step. Changing a step in the middle (`RemoveStep`, `MoveStep`,
`ReplaceStep`) keeps the evaluated steps before it and re-evaluates the rest;
later steps that no longer apply are marked broken. Adding steps and saving
need status `Ready`. The GUI warns before changing a step when later steps
refer to rows by position (cell edits, row insert/delete).

## Full passes

A step needing a full pass leaves the pipeline `Computing`. `compute_job()`
detaches a `ComputeJob` (a pipeline clone) that runs on a worker thread,
reading its input in 64k-row batches and reporting progress; its result is a
cache (kept row ranges for filters; a materialized source for sort/fill) that
`install()` stores if the steps are unchanged (checked by step ids, the job's
`key`). The GUI runs one job at a time after every update, drops a job whose
key went stale, and cancelling undoes the change that started it. Saving
computes pending steps itself.
