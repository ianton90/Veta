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
   Storage      │  veta-core::files                  │
                │  parquet read (in-memory / paged), │
                │  parquet write, CSV/JSON import/   │
                │  export, recipes                   │
                └────────────────────────────────────┘
```

## Workspace layout

```
crates/
  veta-core/   library: model, controller, io, steps, stats. No UI deps, no
               user-facing wording or translation (decision 53).
  veta-fmt/    library: display formatting shared by the GUI and the CLI.
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
  error/                 Error; rejected input as Invalid { code, args,
                         cause } built with invalid!(Code, name = value);
                         codes.rs: ErrorCode and each code's English
  parallel.rs            independent work on all cores
  memory.rs              the memory budget (half of RAM by default)
  profile.rs             timings of the parts of a pass (VETA_PROFILE=1)
  model/                 Document (document/), Workbook, metadata, undo
                         history
  controller/            the only way to change a Document
    command.rs           the Command enum (AddStep adds any step)
    mod.rs               execute / amend / undo / redo, settle (after a
                         full pass); plan() hands each command
                         to its area: cells, nested, metadata, steps
                         (adding steps; columns.rs renames), rows.rs
                         (rows by place, for the GUI and the CLI)
  steps/                 Step, Pipeline (lazy evaluation), transform.rs
                         (the Transform trait every kind of step
                         implements, and the one match over Step),
                         read.rs (caches, then the step's transform),
                         level (shape after a step), full passes
                         (compute: the job a transform's pass runs on),
                         checkpoints (temporary files, private temp
                         folder), cell edits, row steps
  transforms/            what steps do: cast/, filter/, sort/ (in memory,
                         and buckets.rs for paged files), fill, text,
                         split (and merge), columns (add, remove, keep,
                         rename, move, remove if empty), nested/ (values;
                         steps.rs: expand, explode, collect, combine,
                         fields)
  formulas/              custom column expressions: parser, eval, functions
  stats/                 column statistics (summary.rs)
  files/                 parquet.rs (open, save), source/ (in memory /
                         paged), writer/ (parallel Parquet writer),
                         replace.rs (write beside a file, rename over it),
                         recipe/ (JSON), convert/ (CSV, JSON, encodings,
                         export)
  values/                display/ (cell text, types, paths), node (one
                         value and its children, at any depth), parse/
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
                         side_pane/ (steps, stats, file)
  dialogs/               one file per dialog; mod.rs has the shared
                         widgets and the StepDialog trait
  settings/              Settings dialog, config.toml, themes
  platform/              native file dialogs, single instance, system
                         language, icon font
  ribbon/                the ribbon: mod.rs (Actions, tabs, what is
                         enabled), layout.rs (each tab's groups), view.rs
  fonts.rs               installed fonts, interface font and size, text
                         widgets that use them
  i18n/                  translations: mod.rs (tag → language → text, tr),
                         model.rs (step names, option labels, summaries),
                         errors.rs (core errors through ERR_ tags)
veta-gui/translations/   translations_<locale>.json, one per language
veta-gui/tests/          translations: every tag has a text, switching

veta-cli/src/
  lib.rs                 clap definitions, run(), execute()
  commands/              inspect, edit, meta, convert, apply; shared
                         helpers in mod.rs
  table.rs               text tables
  names.rs               English names for steps (the core has none)
veta-cli/tests/cli.rs    runs the real binary

veta-fmt/src/lib.rs      display formatting both front ends share: sizes,
                         shortened values, statistics, alignment
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
   `Command::AddStep(Step::DeleteRows { rows })`.
3. **Command.** The handler passes it to `veta_core::controller::execute`
   (most handlers do so through `App::execute`, which also refreshes the
   grid and reports errors).
4. **Controller.** `plan()` hands it to `controller/steps.rs`, which
   returns a `Change` (here, push the step, its row ranges sorted and
   merged).
   `execute` applies the change to the `Document` and records it in the
   undo history.
5. **Pipeline.** The document's `Pipeline` now has one more step. Nothing is
   computed yet; steps that need a full pass (filter, sort, fill) leave it
   `Computing` and the app runs them on a worker thread.
6. **View.** `App::refresh` tells the tab's `GridView` the data changed;
   the grid reads only the rows on screen (`grid/load.rs`), through the
   pipeline, and `App::view` (`app/view.rs`) draws the window.

Step dialogs (add or rename a column, filter, sort, cast, …) take the same
path through one `Message::Step` handler (`app/steps.rs`) and
`App::add_or_replace`. When a full pass finishes, the app calls
`controller::settle`, which drops a new cast's failed error-column check
(keeping the column) and says so.

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
    that only affect presentation (scroll position, selection, open ribbon
    tab, theme, zoom) change GUI state only: `App` (`app/state.rs`) and each
    tab's `GridView`.
  - CLI: each subcommand builds one or more `Command`s, runs them through
    the controller and prints the result.

## Document model

```
Document
├── path: Option<PathBuf>       // where it is saved; None for new/imported
├── source_file                 // the file the data is read from, and how
├── pipeline: Pipeline          // the source (a DataSource) and the steps
├── info, footer                // the file's layout and Parquet footer
├── metadata: FileMetadata      // key/value metadata, created_by, ...
├── writer: WriterSettings      // row group size, compression, encodings...
├── history: History            // undo/redo stacks of Changes
└── revision                    // bumped when the data shown may change
```

### DataSource
- A trait (`files/source/`): schema, row count, `read(range)`, and
  `read_columns(range, names)` for sources that store columns apart.
- `MemorySource` when the file decodes to no more than the memory budget
  (checked while reading: the footer's sizes aren't trusted, #157).
- `PagedSource` otherwise: chunks of 64K rows within row groups, decoded on
  demand into an LRU cache of a quarter of the budget, the columns of a
  chunk decoded on several cores.
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
- Full passes and saves (save, filter, stats, CSV/JSON export) read the
  output in pieces of 1M rows, several at once on all cores
  (`Pipeline::read_parallel`, results in order); the paged source keeps a
  reader per piece being read and shares the cores between them. The writer
  gathers each row group and encodes every column chunk as a task on a
  pool of threads, several row groups at once (up to 1/8 of the memory
  budget), so few or uneven columns still use all cores.

### Translations
- Translation is the front end's job (decision 53): `veta-core` speaks
  plain English, codes and enums, and another app using it brings its own
  translations.
- `veta_gui::i18n::Translations` is a single shared object (decision 51):
  a map tag → language → text, read from `veta-gui/translations/` (built
  into the binary) at start-up and whenever the language changes. Code
  shows text with `tr("ST_…")`, or `tr_with("ST_…", &[("n", &3)])` for
  texts with `{n}` placeholders. Missing texts fall back to en-GB, then to
  the tag.
- `i18n/model.rs` names the model's values in the app's language: step
  names (from the `Step` data; the core has no step names),
  filter / text / on-error options (`Labeled<T>` for pick lists), value
  summaries and the import report.
- Errors from the core are codes with values (`Error::Invalid`); the GUI
  words them with the tag `ERR_<CODE>` (`errors::error_text`), the CLI
  prints the core's English. A new error: add its code and English to
  `error/codes.rs`, and its `ERR_` texts to every translations file.
- Saving a new language in Settings sends `Message::LanguageChanged`; it
  reloads the translations and the next redraw shows the new texts.
- To add a language: add `translations_<locale>.json` with every tag and
  list it in `i18n/mod.rs`. Tests check every file has every tag and every
  tag in the code has a text.

### Steps
Every data change is a `Step`, Power Query style:

- Cell edits: consecutive edits collapse into one `EditCells` step holding a
  sparse map `(row, column) → value`.
- Row/column changes: `InsertRows`, `DeleteRows`, `AddColumn`,
  `KeepColumns`, `RemoveColumns`, `RenameColumn`, `MoveColumn`,
  `RemoveEmptyColumn`.
- Transformations: `Cast`, `Filter`, `Sort`, `Replace`, `FormatText`,
  `Fill`, `Split`, `Merge`, `CustomColumn` (formulas).
- Nested values: `ExpandStruct`, `ExplodeList`, `AddField`, `RemoveField`,
  `CollectList`, `CombineStruct`.

Each step transforms the output of the previous one. Each kind of step
answers for itself through the `Transform` trait (`steps/transform.rs`):
the shape after it (checking it), how to read a window of its output (by
default, computing it on the same window of its input), whether it needs
a full pass and that pass, and whether it refers to rows by position. The
pipeline only delegates, through `Step::with_transform`, the one match
over every kind of step. A new kind of step needs its `Step` variant and
transform, its recipe form (`files/recipe/`), its names (GUI
`i18n/model.rs`, CLI `names.rs`) and, to edit it, a `StepDialog`.

A list of steps saved
as JSON (`files/recipe/`, a `type` and `settings` per step) is a recipe the
GUI and the CLI can replay.

Reads of some columns (filter tests, statistics, sort keys) read only what
those columns need, down to the file: rows map through row steps, names
through renames, and columns a step doesn't compute pass through it; a
column a step computes (a formula, a cast) needs that step's whole input.

Evaluation is lazy and windowed: to display rows `a..b` the steps are
evaluated only for that window when each is row-preserving (map-like) or
knows where its rows come from (insert / delete rows). Steps that need a
full pass cache their result: a filter its kept row ranges, sort, fill,
collect and explode a materialized source (in memory, or a temporary
Parquet file for paged documents). Paged sorts are external (see
DataSource).

### Undo/redo
The controller turns each `Command` into a `Change` that knows how to apply
and revert itself (e.g. with the previous value of an edited cell, or the
removed step). `History` keeps applied changes on the undo stack; undoing
moves them to the redo stack. Merging consecutive cell edits into one step
is undone one edit at a time.

### Saving
- Data is produced by streaming the evaluated steps piece by piece into the
  writer, so large files never need to fit in memory. Steps still waiting
  for a full pass are computed first; a last filter or sort writes its
  output as it produces it.
- `WriterSettings` is read from the source file's metadata on open (row group
  size, per-column compression/encoding, dictionary, statistics, page size,
  format version) and reused on save. New documents get defaults.
- `ParallelWriter` (`files/writer.rs`) encodes column chunks on all cores.
  It opens the file once the first row group is gathered and turns the
  dictionary off for plain columns whose distinct values there outgrow the
  dictionary page (parquet would drop it for plain values anyway).
- Saving (and export) writes a new, owner-only temp file in the same
  directory and renames it over the target once complete, with the
  target's permissions (`files/replace.rs`, #162); the original is still
  being read during the write. A target that is a link is replaced where
  it points.
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
│   Data grid (virtualized)              │ Steps /         │
│                                        │ Statistics /    │
│                                        │ File            │
├────────────────────────────────────────┴─────────────────┤
│ File tabs: [sales.parquet] [users.parquet] [+]            │
├──────────────────────────────────────────────────────────┤
│ Status bar: rows, columns, memory mode, selection, zoom   │
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
- Fonts apply live: text widgets come from `fonts.rs`, which `App::view`
  sets each frame. Settings lists the installed fonts (read once from
  iced's font database); a saved font that isn't installed is drawn with
  the platform's interface font but kept in the config. The table has its
  own text size (Settings; the interface's by default) times the zoom
  (View → Zoom, Ctrl+wheel), which scales row heights and column widths.

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
`created_with`, `source_file`) and a `steps` array; each step has `order`,
`type` and `settings`. No descriptions: front ends name steps in their own
language (older recipes' `description` fields are ignored).

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
reading its input in batches (the filter in 1M-row pieces on all cores) and
reporting progress; its result is a cache (kept row ranges for filters; a
materialized source for sort, fill, collect and explode) that
`install()` stores if the steps are unchanged (checked by step ids, the job's
`key`). The GUI runs one job at a time after every update, drops a job whose
key went stale, and cancelling undoes the change that started it. Saving
computes pending steps itself.
