//! App tests: opening, saving, closing, import and export.

use super::*;

#[test]
fn opening_adds_and_focuses_tabs() {
    let dir = TempDir::new();
    let a = dir.join("a.parquet");
    let b = dir.join("b.parquet");
    fixtures::all_types(&a);
    fixtures::mixed_settings(&b);

    let app = app_with(&[&a, &b]);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.active, Some(app.tabs[1].id));
    assert_eq!(app.title(), "b.parquet — Veta");
    assert_eq!(app.config.recent_files.len(), 2);
}

#[test]
fn opening_an_open_file_focuses_it() {
    let dir = TempDir::new();
    let a = dir.join("a.parquet");
    let b = dir.join("b.parquet");
    fixtures::all_types(&a);
    fixtures::all_types(&b);
    let mut app = app_with(&[&a, &b]);

    let _ = app.update(Message::FilesPicked(vec![a.clone()]));
    assert_eq!(app.tabs.len(), 2);
    assert!(app.opening.is_empty());
    assert_eq!(app.active, Some(app.tabs[0].id));
}

#[test]
fn closing_focuses_neighbour() {
    let dir = TempDir::new();
    let paths: Vec<_> = (0..3).map(|i| dir.join(&format!("{i}.parquet"))).collect();
    for p in &paths {
        fixtures::all_types(p);
    }
    let mut app = app_with(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>());
    let ids: Vec<_> = app.tabs.iter().map(|t| t.id).collect();

    let _ = app.update(Message::SelectTab(ids[1]));
    let _ = app.update(Message::Action(Action::Close));
    assert_eq!(app.active, Some(ids[2]));
    let _ = app.update(Message::CloseTab(ids[2]));
    assert_eq!(app.active, Some(ids[0]));
    let _ = app.update(Message::CloseTab(ids[0]));
    assert_eq!(app.active, None);
    assert!(app.workbook.is_empty());
}

#[test]
fn open_failure_is_reported() {
    let mut app = app();
    app.opening.push("x.parquet".into());
    let _ = app.update(Message::Opened(
        "x.parquet".into(),
        Err(veta_core::Error::from(std::io::Error::other("boom"))),
    ));
    assert!(app.opening.is_empty());
    assert_eq!(app.errors.len(), 1);
    let _ = app.update(Message::DismissError(0));
    assert!(app.errors.is_empty());
}

#[test]
fn save_in_place_and_save_as() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert!(app.workbook.get(id).unwrap().is_modified());

    // What the worker thread does, run inline.
    let run = |app: &App, target: Option<PathBuf>| {
        let job = app.workbook.get(id).unwrap().save_job(target).unwrap();
        Message::Saved(id, job.run().map(Slot::new))
    };
    app.saving = Some(id);
    let _ = app.update(Message::Action(Action::Undo)); // ignored while saving
    let saved = run(&app, None);
    let _ = app.update(saved);
    assert!(app.saving.is_none());
    let doc = app.workbook.get(id).unwrap();
    assert!(!doc.is_modified());
    assert_eq!(doc.num_rows(), 999);
    assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("1")));

    let copy = dir.join("copy.parquet");
    let saved = run(&app, Some(copy.clone()));
    let _ = app.update(saved);
    assert_eq!(app.title(), "copy.parquet — Veta");
    assert!(copy.exists());
}

#[test]
fn closing_asks_about_unsaved_changes() {
    let dir = TempDir::new();
    let a = dir.join("a.parquet");
    let b = dir.join("b.parquet");
    fixtures::mixed_settings(&a);
    fixtures::mixed_settings(&b);
    let mut app = app_with(&[&a, &b]);
    let (ida, idb) = (app.tabs[0].id, app.tabs[1].id);

    // Unmodified tabs close straight away.
    let _ = app.update(Message::CloseTab(idb));
    assert_eq!(app.tabs.len(), 1);

    let _ = app.update(Message::Grid(
        ida,
        GridEvent::SelectRows {
            row: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(ida, GridEvent::DeleteRows));
    assert!(app.title().starts_with('●'));

    let _ = app.update(Message::Action(Action::Close));
    assert!(matches!(app.dialog, Some(Dialog::ConfirmClose { .. })));
    let _ = app.update(Message::CloseAnswer(CloseAnswer::Cancel));
    assert!(app.dialog.is_none() && app.closing.is_none());
    assert_eq!(app.tabs.len(), 1);

    let _ = app.update(Message::Action(Action::Close));
    let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
    assert!(app.tabs.is_empty());
    assert!(app.dialog.is_none() && app.closing.is_none());
}

#[test]
fn save_then_close() {
    let dir = TempDir::new();
    let a = dir.join("a.parquet");
    fixtures::mixed_settings(&a);
    let mut app = app_with(&[&a]);
    let id = app.tabs[0].id;
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));

    let _ = app.update(Message::CloseTab(id));
    let _ = app.update(Message::CloseAnswer(CloseAnswer::Save));
    assert_eq!(app.saving, Some(id));
    // What the worker thread does.
    let job = app.workbook.get(id).unwrap().save_job(None).unwrap();
    let _ = app.update(Message::Saved(id, job.run().map(Slot::new)));
    assert!(app.tabs.is_empty(), "closed after saving");
    assert_eq!(
        Document::open(&a, OpenOptions::default())
            .unwrap()
            .num_rows(),
        999
    );
}

#[test]
fn quitting_goes_through_every_modified_document() {
    let dir = TempDir::new();
    let paths: Vec<_> = (0..3).map(|i| dir.join(&format!("{i}.parquet"))).collect();
    for p in &paths {
        fixtures::mixed_settings(p);
    }
    let mut app = app_with(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>());
    for id in [app.tabs[0].id, app.tabs[2].id] {
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectRows {
                row: 0,
                extend: false,
            },
        ));
        let _ = app.update(Message::SelectTab(id));
        let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    }
    let _ = app.update(Message::CloseRequested);
    assert!(
        matches!(app.dialog, Some(Dialog::ConfirmClose { document }) if document == app.tabs[0].id)
    );
    let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
    assert!(
        matches!(app.dialog, Some(Dialog::ConfirmClose { document }) if document == app.tabs[2].id)
    );
    let _ = app.update(Message::CloseAnswer(CloseAnswer::Discard));
    assert!(app.dialog.is_none() && app.closing.is_none(), "exits");
}

#[test]
fn import_csv_and_json_then_save_as_parquet() {
    let dir = TempDir::new();
    let csv = dir.join("eu.csv");
    std::fs::write(&csv, "id;price\n1;1,5\n2;2,25\n").unwrap();
    let mut app = app();
    let _ = app.update(Message::ImportPicked(Some(csv.clone())));
    let Some(Dialog::Import(dialog)) = &app.dialog else {
        panic!("CSV files ask how to read them");
    };
    // Semicolons were detected, so decimal commas are assumed.
    let options = dialog.options();
    assert_eq!((options.delimiter, options.decimal), (b';', ','));
    let _ = app.update(Message::Import(ImportMessage::Type(
        0,
        dialogs::CastTarget::Text,
    )));
    let _ = app.update(Message::Import(ImportMessage::Import));
    assert!(app.dialog.is_none());
    assert_eq!(app.opening, std::slice::from_ref(&csv));
    // The worker's result:
    let mut options = options;
    options.types = Some(vec![DataType::Utf8, DataType::Float64]);
    let doc = veta_core::convert::import_csv(&csv, &options, 1 << 30, &mut |_| true).unwrap();
    let _ = app.update(Message::Imported(csv.clone(), Ok(Slot::new(doc))));
    let id = app.tabs[0].id;
    let doc = app.workbook.get(id).unwrap();
    assert_eq!(doc.title(), "eu.csv");
    assert!(doc.is_modified());
    assert_eq!(app.tabs[0].grid.value(1, 1), Some(Some("2.25")));
    assert_eq!(
        files::suggested_name(&doc.title(), FileKind::Parquet),
        "eu.parquet"
    );

    // JSON opens directly in the background.
    let json = dir.join("a.json");
    std::fs::write(&json, "[{\"x\": 1}]").unwrap();
    let _ = app.update(Message::FilesPicked(vec![json.clone()]));
    assert!(app.dialog.is_none());
    assert!(app.opening.contains(&json));
}

#[test]
fn csv_quote_and_custom_separator() {
    let dir = TempDir::new();
    let csv = dir.join("q.txt");
    std::fs::write(&csv, "a#b\n'x#y'#2\n").unwrap();
    let mut dialog = ImportDialog::new(csv);
    dialog.update(ImportMessage::Delimiter(dialogs::Delimiter::Other));
    dialog.update(ImportMessage::Other("ab".into()));
    assert!(
        dialog.update(ImportMessage::Import).is_none(),
        "bad separator"
    );
    dialog.update(ImportMessage::Other("#".into()));
    dialog.update(ImportMessage::Quote(dialogs::Quote::Single));
    let options = dialog.update(ImportMessage::Import).unwrap();
    assert_eq!((options.delimiter, options.quote), (b'#', Some(b'\'')));
}

#[test]
fn import_reports_values_that_did_not_fit() {
    let dir = TempDir::new();
    let csv = dir.join("bad.csv");
    std::fs::write(&csv, "n\n1\nx\n").unwrap();
    let mut app = app();
    let _ = app.update(Message::ImportPicked(Some(csv.clone())));
    let _ = app.update(Message::Import(ImportMessage::Type(
        0,
        dialogs::CastTarget::Int64,
    )));
    let Some(Dialog::Import(dialog)) = &app.dialog else {
        panic!("import dialog");
    };
    let options = dialog.options();
    assert_eq!(options.on_error, OnError::ErrorColumn);
    let result =
        veta_core::convert::import_csv(&csv, &options, 1 << 30, &mut |_| true).map(Slot::new);
    let _ = app.update(Message::Imported(csv, result));
    assert_eq!(app.tabs.len(), 1);
    assert!(app.errors[0].contains("n (1)"), "{:?}", app.errors);
    assert_eq!(app.tabs[0].grid.value(1, 1), Some(Some("x")));
}

#[test]
fn export_dialog() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let _ = app.update(Message::Action(Action::Export));
    let _ = app.update(Message::Export(ExportMessage::Delimiter(
        dialogs::Delimiter::Semicolon,
    )));
    let Some(Dialog::Export { dialog, .. }) = &app.dialog else {
        panic!("export asks for a format");
    };
    assert_eq!(
        dialog.format(),
        veta_core::convert::ExportFormat::Csv(veta_core::convert::CsvExportOptions {
            delimiter: b';',
            decimal: ',',
            ..Default::default()
        })
    );
    let _ = app.update(Message::ExportTo(id, Some(dir.join("out.csv"))));
    assert!(app.dialog.is_none());
    assert_eq!(app.transfers.len(), 1);
    let transfer = app.transfers[0].id;
    let _ = app.update(Message::TransferProgress(transfer, 0.5));
    assert_eq!(app.transfers[0].progress, 0.5);
    let _ = app.update(Message::CancelTransfer(transfer));
    assert!(app.transfers[0].cancel.load(Ordering::Relaxed));
    let _ = app.update(Message::Exported(Ok(dir.join("out.csv"))));
    let _ = app.update(Message::TransferDone(transfer));
    assert!(app.transfers.is_empty());
    assert!(app.notice.as_deref().unwrap().starts_with("Exported"));
}

#[test]
fn save_and_load_recipe() {
    let dir = TempDir::new();
    let a = dir.join("a.parquet");
    let b = dir.join("b.parquet");
    fixtures::mixed_settings(&a);
    fixtures::mixed_settings(&b);
    let mut app = app_with(&[&a, &b]);
    let (ida, idb) = (app.tabs[0].id, app.tabs[1].id);

    let _ = app.update(Message::SelectTab(ida));
    let _ = app.update(Message::Grid(ida, GridEvent::Select { row: 0, column: 0 }));
    let _ = app.update(Message::Action(Action::SortDescending));
    finish_compute(&mut app, ida);
    let _ = app.update(Message::Action(Action::IndexColumn));
    let recipe = dir.join("r-recipe.json");
    let _ = app.update(Message::SaveRecipeTo(ida, Some(recipe.clone())));
    assert!(app.errors.is_empty(), "{:?}", app.errors);

    let _ = app.update(Message::SelectTab(idb));
    let _ = app.update(Message::LoadRecipeFrom(idb, Some(recipe.clone())));
    assert_eq!(app.workbook.get(idb).unwrap().steps().len(), 2);
    finish_compute(&mut app, idb);
    let first = |app: &App, id| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(0..1).unwrap()).unwrap()
    };
    assert_eq!(first(&app, ida), first(&app, idb));

    // Loading it again asks: cancel, add after, or replace.
    let steps = |app: &App| app.workbook.get(idb).unwrap().steps().len();
    let _ = app.update(Message::LoadRecipeFrom(idb, Some(recipe.clone())));
    assert!(matches!(app.dialog, Some(Dialog::ConfirmRecipe { .. })));
    let _ = app.update(Message::RecipeChoice(None));
    assert_eq!((steps(&app), app.dialog.is_none()), (2, true));
    let _ = app.update(Message::LoadRecipeFrom(idb, Some(recipe.clone())));
    let _ = app.update(Message::RecipeChoice(Some(true)));
    assert_eq!(steps(&app), 2, "replaced, not stacked");
    finish_compute(&mut app, idb);
    assert_eq!(first(&app, ida), first(&app, idb));
    let _ = app.update(Message::LoadRecipeFrom(idb, Some(recipe)));
    let _ = app.update(Message::RecipeChoice(Some(false)));
    assert!(steps(&app) > 2, "added after");

    let _ = app.update(Message::LoadRecipeFrom(idb, Some(dir.join("missing.toml"))));
    assert_eq!(app.errors.len(), 1);
}

#[test]
fn new_file_takes_columns_and_rows() {
    let mut app = app();
    let _ = app.update(Message::Action(Action::New));
    let _ = app.update(Message::Action(Action::New));
    let titles: Vec<String> = app.workbook.iter().map(|(_, d)| d.title()).collect();
    assert_eq!(titles, ["Untitled", "Untitled 2"]);
    let id = app.active.unwrap();
    assert!(!app.workbook.get(id).unwrap().is_modified());

    let blank = |app: &mut App| {
        app.menu = Some(ContextMenu {
            document: id,
            target: MenuTarget::Blank,
            position: iced::Point::ORIGIN,
        });
    };
    blank(&mut app);
    let _ = app.update(Message::Menu(MenuItem::AddColumn));
    let _ = app.update(Message::Step(StepMessage::Column(ColumnMessage::Name(
        "a".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Column(ColumnMessage::Submit)));
    blank(&mut app);
    let _ = app.update(Message::Menu(MenuItem::AddRow));
    blank(&mut app);
    let _ = app.update(Message::Menu(MenuItem::AddRow));
    let doc = app.workbook.get(id).unwrap();
    assert_eq!((doc.num_rows(), doc.num_columns()), (2, 1));
    assert_eq!(app.selected_rows(id), Some(1..2));
    assert!(doc.path().is_none(), "Save asks for a path");
}
