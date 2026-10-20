//! App tests: column commands and their dialogs.

use super::*;

#[test]
fn column_commands() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let names = |app: &App| -> Vec<String> {
        let doc = app.workbook.get(id).unwrap();
        doc.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    };

    // Insert a column right of "name" through the menu and dialog.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    app.menu = Some(ContextMenu {
        document: id,
        target: MenuTarget::Column(1),
        position: iced::Point::ORIGIN,
    });
    let _ = app.update(Message::Menu(MenuItem::InsertColumnRight));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("note".into())));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app), ["id", "name", "note", "score", "flag"]);
    assert_eq!(app.selected_columns(id), Some(2..3));

    // Rename with a clashing name keeps the dialog open.
    let _ = app.update(Message::Action(Action::RenameColumn));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("id".into())));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
    assert!(app.dialog.is_some());
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Name("memo".into())));
    let _ = app.update(Message::ColumnDialog(ColumnMessage::Submit));
    assert_eq!(names(&app), ["id", "name", "memo", "score", "flag"]);

    // Drag "memo" to the front, then remove two columns.
    let _ = app.update(Message::Grid(id, GridEvent::MoveColumn { from: 2, to: 0 }));
    assert_eq!(names(&app), ["memo", "id", "name", "score", "flag"]);
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 3,
            extend: true,
        },
    ));
    assert_eq!(app.selected_columns(id), Some(0..4));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 3,
            extend: false,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 4,
            extend: true,
        },
    ));
    let _ = app.update(Message::Action(Action::RemoveColumns));
    assert_eq!(names(&app), ["memo", "id", "name"]);

    // With one column selected, the ribbon asks; the dialog keeps the
    // checked ones.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::KeepColumns));
    assert!(matches!(app.dialog, Some(Dialog::Choose { .. })));
    let _ = app.update(Message::Choose(ChooseMessage::Toggle(2, true)));
    let _ = app.update(Message::Choose(ChooseMessage::Apply));
    assert_eq!(names(&app), ["id", "name"]);

    // The column menu keeps the selection directly.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::ContextMenu {
            target: MenuTarget::Column(1),
            position: iced::Point::ORIGIN,
        },
    ));
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Menu(MenuItem::KeepColumns));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app), ["name"]);

    while app.workbook.get(id).unwrap().history().can_undo() {
        let _ = app.update(Message::Action(Action::Undo));
    }
    assert_eq!(names(&app), ["id", "name", "score", "flag"]);
}

#[test]
fn combine_columns_into_a_struct() {
    let dir = TempDir::new();
    let path = dir.join("n.parquet");
    fixtures::nested(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let names = |app: &App| -> Vec<String> {
        let doc = app.workbook.get(id).unwrap();
        doc.schema()
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect()
    };
    let select = |app: &mut App, column, extend| {
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns { column, extend },
        ));
    };
    // Expanded fields go back into their struct by default.
    select(&mut app, 1, false);
    let _ = app.update(Message::Action(Action::ExpandStruct));
    select(&mut app, 1, false);
    select(&mut app, 2, true);
    let _ = app.update(Message::Action(Action::CombineStruct));
    let _ = app.update(Message::Step(StepMessage::Combine(CombineMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app)[..3], ["id", "point", "tags"]);

    // Other columns get a new name; a taken one keeps the dialog open.
    select(&mut app, 0, false);
    select(&mut app, 2, true);
    let _ = app.update(Message::Action(Action::CombineStruct));
    let _ = app.update(Message::Step(StepMessage::Combine(CombineMessage::Name(
        "owner".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Combine(CombineMessage::Apply)));
    assert!(step_dialog::<CombineDialog>(&app).is_some());
    let _ = app.update(Message::Step(StepMessage::Combine(CombineMessage::Name(
        "rec".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Combine(CombineMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app), ["rec", "path", "scores", "owner"]);

    // The step reopens in its dialog.
    let _ = app.update(Message::EditStep(2));
    assert!(step_dialog::<CombineDialog>(&app).is_some());
}

#[test]
fn collect_columns_into_lists() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    // Collect score, grouped by name (the second of the others).
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 2,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::CollectList));
    let _ = app.update(Message::Step(StepMessage::Collect(
        CollectMessage::ToggleKey(1, true),
    )));
    let _ = app.update(Message::Step(StepMessage::Collect(CollectMessage::Apply)));
    assert!(app.dialog.is_none());
    finish_compute(&mut app, id);
    let doc = app.workbook.get(id).unwrap();
    let names: Vec<_> = doc
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["name", "score"]);
    assert_eq!(doc.num_rows(), 6);
    assert_eq!(
        doc.steps()[0].describe(),
        "Collected score into lists by name"
    );

    // The step reopens with its group columns checked.
    let _ = app.update(Message::EditStep(0));
    let Some(dialog) = step_dialog::<CollectDialog>(&app) else {
        panic!("collect dialog");
    };
    assert_eq!(
        dialog,
        &CollectDialog::new(
            vec!["score".into()],
            vec![
                ("id".into(), false),
                ("name".into(), true),
                ("flag".into(), false)
            ]
        )
    );
    // It names the columns the result leaves out.
    assert_eq!(dialog.dropped(), ["id", "flag"]);
}

#[test]
fn new_list_and_struct_columns() {
    use crate::dialogs::{TypeChoice, TypeOption, TypeStep};
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let column = |app: &mut App, message| {
        let _ = app.update(Message::ColumnDialog(message));
    };
    let type_of = |app: &App, name: &str| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::type_name(doc.schema().field_with_name(name).unwrap().data_type())
    };

    let _ = app.update(Message::Action(Action::AddColumn));
    column(&mut app, ColumnMessage::Name("tags".into()));
    column(&mut app, ColumnMessage::Type(vec![], TypeOption::List));
    column(
        &mut app,
        ColumnMessage::Type(vec![TypeStep::Item], TypeOption::Plain(TypeChoice::Integer)),
    );
    column(&mut app, ColumnMessage::Submit);
    assert!(app.dialog.is_none());
    assert_eq!(type_of(&app, "tags"), "list<int64>");

    let _ = app.update(Message::Action(Action::AddColumn));
    column(&mut app, ColumnMessage::Name("point".into()));
    column(&mut app, ColumnMessage::Type(vec![], TypeOption::Struct));
    column(&mut app, ColumnMessage::FieldName(vec![], 0, "x".into()));
    column(
        &mut app,
        ColumnMessage::Type(
            vec![TypeStep::Field(0)],
            TypeOption::Plain(TypeChoice::Decimal),
        ),
    );
    column(&mut app, ColumnMessage::AddField(vec![]));
    column(&mut app, ColumnMessage::FieldName(vec![], 1, "x".into()));
    column(&mut app, ColumnMessage::Submit);
    assert!(app.dialog.is_some(), "two fields named x");
    column(&mut app, ColumnMessage::FieldName(vec![], 1, "y".into()));
    column(&mut app, ColumnMessage::AddField(vec![]));
    column(&mut app, ColumnMessage::RemoveField(vec![], 2));
    column(&mut app, ColumnMessage::Submit);
    assert!(app.dialog.is_none());
    assert_eq!(type_of(&app, "point"), "struct<x: float64, y: string>");

    // Editing the step shows the struct's fields again.
    let _ = app.update(Message::EditStep(1));
    let Some(Dialog::Column { dialog, .. }) = &app.dialog else {
        panic!("column dialog");
    };
    let debug = format!("{dialog:?}");
    assert!(debug.contains("spec: Struct"), "{debug}");
    assert!(debug.contains(r#"("y", Plain(Text))"#), "{debug}");

    // Types nested to any depth: a list of structs holding a map of lists.
    let _ = app.update(Message::Action(Action::AddColumn));
    column(&mut app, ColumnMessage::Name("orders".into()));
    column(&mut app, ColumnMessage::Type(vec![], TypeOption::List));
    let item = vec![TypeStep::Item];
    column(
        &mut app,
        ColumnMessage::Type(item.clone(), TypeOption::Struct),
    );
    column(
        &mut app,
        ColumnMessage::FieldName(item.clone(), 0, "sku".into()),
    );
    column(&mut app, ColumnMessage::AddField(item.clone()));
    column(
        &mut app,
        ColumnMessage::FieldName(item.clone(), 1, "options".into()),
    );
    let options = vec![TypeStep::Item, TypeStep::Field(1)];
    column(
        &mut app,
        ColumnMessage::Type(options.clone(), TypeOption::Map),
    );
    let values = vec![TypeStep::Item, TypeStep::Field(1), TypeStep::Item];
    column(&mut app, ColumnMessage::Type(values, TypeOption::List));
    column(&mut app, ColumnMessage::Submit);
    assert!(app.dialog.is_none());
    assert_eq!(
        type_of(&app, "orders"),
        "list<struct<sku: string, options: map<string, list<string>>>>"
    );
    let _ = app.update(Message::EditStep(2));
    let Some(Dialog::Column { dialog, .. }) = &app.dialog else {
        panic!("column dialog");
    };
    assert!(format!("{dialog:?}").contains(r#"("options", Map(Text, List(Plain(Text))))"#));
}

#[test]
fn change_type_dialog_and_edit_step() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let schema = |app: &App| app.workbook.get(id).unwrap().schema();

    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::DataType));
    assert!(step_dialog::<CastDialog>(&app).is_some());
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::Type(
        dialogs::CastTarget::Int64,
    ))));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::OnError(
        veta_core::steps::OnError::ErrorColumn,
    ))));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::Apply)));
    assert!(app.dialog.is_none());
    // The check finds errors, so it's dropped and the errors kept.
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 2);
    finish_compute(&mut app, id);
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);
    assert_eq!(schema(&app).field(1).data_type(), &DataType::Int64);
    assert_eq!(schema(&app).field(2).name(), "name errors");
    assert!(
        app.errors
            .iter()
            .any(|e| e.contains("kept in column name errors"))
    );

    // Without errors the error column is removed.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::DataType));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::Type(
        dialogs::CastTarget::Text,
    ))));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::OnError(
        veta_core::steps::OnError::ErrorColumn,
    ))));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::Apply)));
    finish_compute(&mut app, id);
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 3);
    assert_eq!(schema(&app).fields().len(), 5);
    let _ = app.update(Message::Action(Action::Undo));
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);

    // Edit it: back to null on error, so the error column goes away.
    let _ = app.update(Message::EditStep(0));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::OnError(
        veta_core::steps::OnError::Null,
    ))));
    let _ = app.update(Message::Step(StepMessage::Cast(CastMessage::Apply)));
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 1);
    assert_eq!(schema(&app).field(2).name(), "score");
}

#[test]
fn replace_fill_and_format() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let cell = |app: &App, row: usize, column: usize| {
        let doc = app.workbook.get(id).unwrap();
        veta_core::display::format_batch(&doc.read(row..row + 1).unwrap()).unwrap()[0][column]
            .clone()
    };
    let select_column = |app: &mut App, column: usize| {
        let _ = app.update(Message::Grid(
            id,
            GridEvent::SelectColumns {
                column,
                extend: false,
            },
        ));
    };

    // Replace in score: an invalid number keeps the dialog open.
    select_column(&mut app, 2);
    let _ = app.update(Message::Action(Action::ReplaceValues));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::Find(
        "x".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::With(
        "1".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::Apply)));
    assert!(step_dialog::<ReplaceDialog>(&app).is_some());
    let _ = app.update(Message::Step(StepMessage::Replace(
        ReplaceMessage::FindNull(true),
    )));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(cell(&app, 0, 2).as_deref(), Some("1.0"));

    // Fill down names; uppercase them.
    select_column(&mut app, 1);
    let _ = app.update(Message::Action(Action::FillDown));
    finish_compute(&mut app, id);
    assert_eq!(cell(&app, 7, 1).as_deref(), Some("bo"));
    let _ = app.update(Message::Action(Action::FormatText));
    let _ = app.update(Message::Step(StepMessage::Format(FormatMessage::Op(
        TextOp::Upper,
    ))));
    let _ = app.update(Message::Step(StepMessage::Format(FormatMessage::Apply)));
    assert_eq!(cell(&app, 7, 1).as_deref(), Some("BO"));

    // Edit the replace step: later steps aren't positional, no warning.
    let _ = app.update(Message::EditStep(0));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::With(
        "2".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Replace(ReplaceMessage::Apply)));
    assert!(app.dialog.is_none());
    finish_compute(&mut app, id);
    assert_eq!(cell(&app, 0, 2).as_deref(), Some("2.0"));
    assert_eq!(app.workbook.get(id).unwrap().steps().len(), 3);
}

#[test]
fn split_and_merge_columns() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let names = |app: &App| {
        let schema = app.workbook.get(id).unwrap().schema();
        schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect::<Vec<_>>()
    };

    // Merge id and name with "-".
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::MergeColumns));
    assert!(app.dialog.is_none(), "one column can't be merged");
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 1,
            extend: true,
        },
    ));
    let _ = app.update(Message::Action(Action::MergeColumns));
    let _ = app.update(Message::Step(StepMessage::Merge(MergeMessage::Separator(
        "-".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Merge(MergeMessage::Name(
        "score".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Merge(MergeMessage::Apply)));
    assert!(
        step_dialog::<MergeDialog>(&app).is_some(),
        "a clashing name keeps the dialog open"
    );
    let _ = app.update(Message::Step(StepMessage::Merge(MergeMessage::Name(
        "key".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Merge(MergeMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app), ["key", "score", "flag"]);

    // Split it back; the dialog suggests 2 parts.
    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 0,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::SplitColumn));
    let _ = app.update(Message::Step(StepMessage::Split(SplitMessage::Delimiter(
        "-".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Split(SplitMessage::Apply)));
    assert_eq!(names(&app), ["key.1", "key.2", "score", "flag"]);

    // Edit the split: by position.
    let _ = app.update(Message::EditStep(1));
    let _ = app.update(Message::Step(StepMessage::Split(SplitMessage::Mode(
        dialogs::SplitMode::Positions,
    ))));
    let _ = app.update(Message::Step(StepMessage::Split(SplitMessage::Positions(
        "1, 2".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Split(SplitMessage::Apply)));
    assert_eq!(names(&app), ["key.1", "key.2", "key.3", "score", "flag"]);
}

#[test]
fn custom_duplicate_and_index_columns() {
    let dir = TempDir::new();
    let path = dir.join("m.parquet");
    fixtures::mixed_settings(&path);
    let mut app = app_with(&[&path]);
    let id = app.tabs[0].id;
    let schema = |app: &App| app.workbook.get(id).unwrap().schema();
    let names = |app: &App| {
        schema(app)
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect::<Vec<_>>()
    };

    let _ = app.update(Message::Grid(
        id,
        GridEvent::SelectColumns {
            column: 2,
            extend: false,
        },
    ));
    let _ = app.update(Message::Action(Action::CustomColumn));
    let _ = app.update(Message::Step(StepMessage::Custom(
        CustomMessage::Expression("[score] * \"x\"".into()),
    )));
    let _ = app.update(Message::Step(StepMessage::Custom(CustomMessage::Apply)));
    assert!(
        step_dialog::<CustomDialog>(&app).is_some(),
        "an invalid formula keeps the dialog open"
    );
    let _ = app.update(Message::Step(StepMessage::Custom(
        CustomMessage::Expression("score * 2".into()),
    )));
    let _ = app.update(Message::Step(StepMessage::Custom(CustomMessage::Name(
        "double".into(),
    ))));
    let _ = app.update(Message::Step(StepMessage::Custom(CustomMessage::Apply)));
    assert!(app.dialog.is_none());
    assert_eq!(names(&app), ["id", "name", "score", "double", "flag"]);

    let _ = app.update(Message::Action(Action::DuplicateColumn));
    let _ = app.update(Message::Action(Action::DuplicateColumn));
    assert_eq!(names(&app)[3], "score - Copy 2");
    assert_eq!(schema(&app).field(3).data_type(), &DataType::Float64);
    let _ = app.update(Message::Action(Action::IndexColumn));
    assert_eq!(names(&app).last().map(String::as_str), Some("Index"));

    // Edit the first custom column.
    let _ = app.update(Message::EditStep(0));
    let _ = app.update(Message::Step(StepMessage::Custom(
        CustomMessage::Expression("text(score)".into()),
    )));
    let _ = app.update(Message::Step(StepMessage::Custom(CustomMessage::Apply)));
    assert_eq!(schema(&app).field(3 + 2).data_type(), &DataType::Utf8);
}
