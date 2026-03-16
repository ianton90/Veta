# Veta — Developer Guidelines

## App Overview

Veta is a cross-platform desktop parquet file editor built with Avalonia.

### Layout
- **Top menu bar** — file operations, preferences, and general app actions
- **Collapsible ribbon** — MS Office-style, contains both file operations and data transformation tools organized in groups
- **Tabbed interface** — one tab per open file

### ETL Pipeline Model
Every open file is backed by an ETL pipeline:
- The first step is always a data source (open a parquet file, or import from another source via a wizard)
- Every modification to the data is recorded as a step in the pipeline
- The DataGrid always displays the output of the last step
- Users can navigate back and forth through steps in order
- If a user edits a step that has subsequent steps, they are warned and all subsequent steps are discarded

### Manual Edits
Multiple manual cell edits can be combined into a single pipeline step.

## Coding Conventions

Follow the [Microsoft C# Coding Conventions](https://learn.microsoft.com/en-us/dotnet/csharp/fundamentals/coding-style/coding-conventions) with the exceptions listed below.

### Exceptions to Microsoft Conventions
- Always use explicit types, never `var`

## Libraries

| Library | NuGet Package | Purpose |
|---|---|---|
| Avalonia | `Avalonia` | UI framework |
| Parquet.Net | `Parquet.Net` | Read and write Parquet files |
| Microsoft.Data.Analysis | `Microsoft.Data.Analysis` | In-memory data manipulation (DataFrame) |
| Avalonia.Controls.DataGrid | `Avalonia.Controls.DataGrid` | Spreadsheet-like table control for editing data |

## Rules

### Architecture
- No logic in code-behind files (`.axaml.cs`) — everything goes in ViewModels
- No direct UI manipulation from ViewModels — use bindings
- No business logic in Views
- No direct references from ViewModels to Views

### Code Style
- No unnecessary comments — code should be self-explanatory
- No over-engineering — don't add abstractions until they're needed
- No premature optimization
- No ceremony — prefer flat, direct code over intermediate variables and nested assignments

## Localization

- All user-visible strings must be localized — no hardcoded strings in Views or ViewModels
- Translation files are JSON, one per language: `Resources/Translations/Translation.en.json`, `Resources/Translations/Translation.es.json`, etc.
- The `Localizer` service (in `Services/Localizer.cs`) auto-detects the system language and loads the appropriate file, falling back to English
- Access strings in ViewModels via `Localizer.Get("KeyName")`
- In XAML, bind via the ViewModel — expose localized strings as ViewModel properties

## Files
- One class per file
- File name must match the class name (e.g. `ParquetFile.cs` contains `class ParquetFile`)

