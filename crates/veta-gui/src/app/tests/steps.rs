use super::*;

#[test]
fn applied_steps_preview_edit_and_warnings() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let doc = |app: &App| app.workbook.get(id).unwrap().steps().len();

    // Rename "name" → "label", then delete row 1.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::RenameColumn));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("label".into())));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectRows {
            row: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert_eq!(doc(&app), 2);

    // Preview the source: read-only.
    let _ = app.update(Message::PreviewStep(id, 0));
    assert_eq!(app.tabs[0].grid.preview(), Some(0));
    assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("0")));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert_eq!(doc(&app), 2, "changes are disabled while previewing");
    assert_eq!(app.errors.len(), 1);
    let _ = app.update(Message::PreviewStep(id, usize::MAX));
    assert_eq!(app.tabs[0].grid.preview(), None);

    // Editing the rename asks first, because a later step deletes rows
    // by position.
    let _ = app.update(Message::EditStep(0));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("title".into())));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
    assert!(matches!(app.dialog, Some(Dialog::ConfirmStep { .. })));
    let _ = app.update(Message::ConfirmStep(true));
    let schema = app.workbook.get(id).unwrap().schema();
    assert_eq!(schema.field(1).name(), "title");

    // Removing the last step doesn't ask.
    let _ = app.update(Message::RemoveStep(1));
    assert!(app.dialog.is_none());
    assert_eq!(doc(&app), 1);
}

#[test]
fn filter_view_then_keep_or_remove() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let n = fixtures::MIXED_SETTINGS_ROWS;
    let rows = |app: &App| app.workbook.get(id).unwrap().num_rows();
    let steps = |app: &App| app.workbook.get(id).unwrap().steps().len();
    let shown = |app: &App| {
        let doc = app.workbook.get(id).unwrap();
        app.tabs[0].grid.filter_counts(doc).map(|c| c.0)
    };
    let anas = (0..n).filter(|i| i % 5 == 0 && i % 7 != 0).count();

    // Filter opens on the selected column and sets a view, not a step.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::FilterRows));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Op(
        0,
        FilterOp::Greater,
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Column(
        0,
        "score".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Value(
        0,
        "abc".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Apply)));
    assert!(
        step_dialog::<FilterDialog>(&app).is_some(),
        "an invalid value keeps the dialog open"
    );
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Column(
        0,
        "name".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Op(
        0,
        FilterOp::Equals,
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Value(
        0,
        "ANA".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(steps(&app), 0);
    assert!(
        app.computing
            .as_ref()
            .is_some_and(|c| c.target == Target::View)
    );
    finish_filter(&mut app, id);
    assert_eq!(shown(&app), Some(anas));
    assert_eq!(app.tabs[0].grid.value(0, 1), Some(Some("ana")));

    // Read-only while filtered.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 0 }));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    assert_eq!(steps(&app), 0);
    assert!(!app.errors.is_empty());
    app.errors.clear();

    // Filter by a cell value adds to the filter.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 3 }));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Cell,
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::ExcludeValue));
    assert_eq!(app.tabs[0].grid.filter().unwrap().conditions.len(), 2);
    finish_filter(&mut app, id);
    let first_flag = app.tabs[0].grid.value(0, 3).flatten().map(str::to_owned);
    assert!(first_flag.is_some());

    // Remove rows: the inverse of what's shown becomes a step.
    let shown_now = shown(&app).unwrap();
    let _ = app.update(Message::Action(Action::RemoveRows));
    assert!(app.tabs[0].grid.filter().is_none());
    finish_compute(&mut app, id);
    assert_eq!(rows(&app), n - shown_now);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(rows(&app), n);

    // Clear drops the view; Keep keeps the rows shown.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 1, column: 1 }));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Cell,
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::KeepValue));
    finish_filter(&mut app, id);
    let _ = app.update(Message::ClearFilter);
    assert!(app.tabs[0].grid.filter().is_none());
    assert_eq!(app.tabs[0].grid.value(0, 0), Some(Some("0")));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Cell,
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::KeepValue));
    finish_filter(&mut app, id);
    let _ = app.update(Message::Action(Action::KeepRows));
    finish_compute(&mut app, id);
    let bos = (0..n).filter(|i| i % 5 == 1 && i % 7 != 0).count();
    assert_eq!(rows(&app), bos);

    // Cancel while filtering clears the filter.
    let _ = app.update(Message::Action(Action::FilterRows));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Value(
        0,
        "x".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Apply)));
    assert!(app.computing.is_some());
    let _ = app.update(Message::CancelCompute);
    assert!(app.tabs[0].grid.filter().is_none());
    assert_eq!(steps(&app), 1);

    // Editing the filter step still works through the dialog.
    let _ = app.update(Message::EditStep(0));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Value(
        0,
        "ana".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Filter(FilterMessage::Apply)));
    finish_compute(&mut app, id);
    assert_eq!(rows(&app), anas);
}

#[test]
fn no_edits_while_computing_but_other_files_work() {
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
    let steps = |app: &App, id| app.workbook.get(id).unwrap().steps().len();
    assert_eq!(steps(&app, ida), 1);

    let attempts = [
        Message::Grid(
            ida,
            GridEvent::StartEdit {
                initial: Some("5".into()),
            },
        ),
        Message::EditInput("5".into()),
        Message::Grid(ida, GridEvent::ClearCell),
        Message::Grid(ida, GridEvent::DeleteRows),
        Message::Grid(ida, GridEvent::InsertRows),
        Message::Grid(ida, GridEvent::MoveColumn { from: 0, to: 1 }),
        Message::Action(Action::RemoveRows),
        Message::Action(Action::RemoveColumns),
        Message::Action(Action::SortAscending),
        Message::Action(Action::Undo),
        Message::RemoveStep(0),
        Message::EditStep(0),
        Message::LoadRecipe,
    ];
    for message in attempts {
        let _ = app.update(message.clone());
        assert!(app.edit.is_none(), "{message:?}");
        assert!(app.dialog.is_none(), "{message:?}");
        assert_eq!(steps(&app, ida), 1, "{message:?}");
        assert!(app.computing.is_some(), "{message:?}");
    }
    // The column menu's actions are refused too.
    let _ = app.update(Message::Grid(
        ida,
        GridEvent::ContextMenu {
            target: MenuTarget::Column(0),
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::RemoveColumns));
    assert_eq!(steps(&app, ida), 1);

    // The other file can be edited meanwhile.
    let _ = app.update(Message::SelectTab(idb));
    let _ = app.update(Message::Grid(idb, GridEvent::Select { row: 0, column: 0 }));
    let _ = app.update(Message::Action(Action::RemoveColumns));
    assert!(matches!(app.dialog, Some(Dialog::Choose { .. })));
    let _ = app.update(Message::CloseDialog);
    let _ = app.update(Message::Grid(idb, GridEvent::Select { row: 1, column: 1 }));
    let _ = app.update(Message::Grid(idb, GridEvent::ClearCell));
    assert_eq!(steps(&app, idb), 1);

    let _ = app.update(Message::SelectTab(ida));
    finish_compute(&mut app, ida);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(steps(&app, ida), 0);
}

#[test]
fn statistics_pane_follows_selection_and_edits() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;

    // Hidden: nothing computed.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 0 }));
    assert!(app.stats.running.is_none());

    let _ = app.update(Message::Action(Action::StatisticsPane));
    assert_eq!(app.side_tab, SideTab::Stats);
    // Quick stats from the footer while the pass runs.
    let view = app.stats_view(id);
    assert_eq!(view.columns.len(), 1);
    let (name, quick, partial) = &view.columns[0];
    assert_eq!(name, "id");
    assert!(*partial);
    assert_eq!(quick.as_ref().unwrap().max.as_deref(), Some("999"));
    assert_eq!(app.stats.running.as_ref().unwrap().columns, ["id"]);
    finish_stats(&mut app);
    let view = app.stats_view(id);
    let (_, full, partial) = &view.columns[0];
    assert!(!partial);
    assert_eq!(full.as_ref().unwrap().distinct, Some(1000));

    // Selecting more columns computes only the missing ones.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: true,
        },
    ));
    assert_eq!(app.stats.running.as_ref().unwrap().columns, ["name"]);
    finish_stats(&mut app);
    assert!(app.stats.running.is_none());

    // After an edit the statistics are recomputed (once the step is).
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
    let _ = app.update(Message::Grid(id, GridEvent::DeleteRows));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 0,
            extend: false,
        },
    ));
    finish_stats(&mut app);
    let view = app.stats_view(id);
    assert_eq!(view.columns[0].1.as_ref().unwrap().count, 999);
    assert!(
        app.stats
            .cache
            .keys()
            .all(|(_, r, _)| *r == app.workbook.get(id).unwrap().revision())
    );

    // Switching tabs stops a pass.
    let _ = app.update(Message::Action(Action::Undo));
    assert!(app.stats.running.is_some());
    let _ = app.update(Message::SideTab(SideTab::Steps));
    assert!(app.stats.running.is_none());
}

#[test]
fn sort_from_ribbon_and_edit_step() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let id_at = |app: &App, row: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][0].clone()
    };

    // Opening a file starts on the first cell (#90).
    assert_eq!(app.tabs[0].grid.selected(), Some((0, 0)));
    let _ = app.update(Message::Action(Action::SortDescending));
    finish_compute(&mut app, id);
    assert_eq!(id_at(&app, 0).as_deref(), Some("999"));

    // Custom sort: score descending with nulls first, as a new step.
    let _ = app.update(Message::Grid(id, GridEvent::Select { row: 0, column: 2 }));
    let _ = app.update(Message::Action(Action::CustomSort));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Direction(
        0,
        dialogs::Direction::Descending,
    ))));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Nulls(
        0,
        dialogs::Nulls::First,
    ))));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Apply)));
    finish_compute(&mut app, id);
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 2);
    let doc = app.workbook.get(id).unwrap();
    let first = veta_core::display::format_batch(&doc.read(0..1).unwrap()).unwrap();
    assert_eq!(first[0][2], None, "nulls first");
    let _ = app.update(Message::Action(Action::Undo));

    // Edit it into a two-key sort: flag, then id ascending.
    let _ = app.update(Message::EditStep(0));
    assert!(step_dialog::<SortDialog>(&app).is_some());
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Direction(
        0,
        dialogs::Direction::Ascending,
    ))));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Add)));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Column(
        0,
        "flag".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Column(
        1,
        "id".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Sort(SortMessage::Apply)));
    assert!(app.dialog.is_none());
    finish_compute(&mut app, id);
    assert_eq!(id_at(&app, 0).as_deref(), Some("1"));
    assert_eq!(id_at(&app, 500).as_deref(), Some("0"));
}
