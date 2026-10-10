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
  cache bounded by the memory budget.
- Both expose the same trait: schema, row count, `read(range) -> RecordBatch`.

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

## GUI layout

```
┌──────────────────────────────────────────────────────────┐
│ Ribbon: [Home] [Transform] [Add Column] [View] ...        │
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
- Themes: TOML files with font family/size, palette and light/dark flag,
  converted to an iced `Theme` plus Veta's own style tokens.

## CLI

```
veta [FILE]...                         # open GUI (with files)
veta open <file>...                    # same
veta info | schema <file>
veta head <file> [-n N]
veta set-cell <file> --row R --column C (--value V | --null)
veta insert-rows <file> --at R [--count N]
veta delete-rows <file> --rows 5,10-20
veta add-column <file> <name> --type T [--at P]
veta remove-column <file> <name>...
veta rename-column <file> <from> <to>
veta move-column <file> <name> --to P
veta meta list|get|set|remove <file> [key [value]]
veta writer-settings <file> [--column C]... [--compression ...] [--level N]
      [--encoding ...] [--dictionary on|off] [--statistics none|chunk|page]
      [--bloom-filter on|off] [--row-group-rows N] [--format-version 1|2]
```

Rows and positions are numbered from 1, as in the GUI. Commands that modify
write in place unless `-o FILE` is given. Planned: `stats`, `apply`,
`convert`.

## Testing
- `veta-core`: unit tests per module; integration tests on small parquet
  fixtures generated in tests (no binary fixtures committed unless needed).
- `veta-cli`: end-to-end tests running subcommands on temp files.
- `veta-gui`: logic tests on `update`; visual checks are manual.

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
