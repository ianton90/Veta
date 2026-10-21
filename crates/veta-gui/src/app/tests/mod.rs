//! App tests, by area, driven through `App::update` as the window would.
//! Shared helpers and general tests (theme, settings, menus) are here.

use super::*;
use crate::dialogs::{
    CastMessage, CollectMessage, CombineMessage, CustomMessage, FilterMessage, FormatMessage,
    MergeMessage, ReplaceMessage, SortMessage, SplitMessage,
};
use veta_core::arrow::datatypes::DataType;
use veta_testkit::{TempDir, fixtures};

mod clipboard;
mod columns;
mod documents;
mod editing;
mod nested;
mod rows;
mod steps;

fn app() -> App {
    let (themes, _) = theme::load_all(None);
    // English whatever the system language, so texts can be checked.
    let mut config = Config::default();
    config.appearance.language = Some("en-GB".into());
    let startup = Startup {
        config,
        config_path: None,
        themes,
        themes_dir: None,
        errors: Vec::new(),
    };
    App::new(startup, Vec::new()).0
}

fn app_with(paths: &[&Path]) -> App {
    let mut app = app();
    for path in paths {
        let doc = Document::open(path, OpenOptions::default()).unwrap();
        let _ = app.update(Message::Opened(path.to_path_buf(), Ok(Loaded::new(doc))));
    }
    app
}

/// Runs the pending pass of document `id` here, as the worker would.
fn finish_compute(app: &mut App, id: DocumentId) {
    let job = app.workbook.get(id).unwrap().compute_job().unwrap();
    let key = job.key().to_vec();
    let result = job
        .run(&mut |_| true)
        .map(Slot::new)
        .map_err(|e| e.to_string());
    let _ = app.update(Message::ComputeDone(id, Target::Document, key, result));
}

/// Runs the pending filter of document `id`'s tab here.
fn finish_filter(app: &mut App, id: DocumentId) {
    let tab = app.tabs.iter().find(|t| t.id == id).unwrap();
    let job = tab.grid.filter_job().unwrap();
    let key = job.key().to_vec();
    let result = job
        .run(&mut |_| true)
        .map(Slot::new)
        .map_err(|e| e.to_string());
    let _ = app.update(Message::ComputeDone(id, Target::View, key, result));
}

/// Runs the pending statistics pass here, as the worker would.
fn finish_stats(app: &mut App) {
    let run = app.stats.running.as_ref().unwrap();
    let (id, revision) = (run.document, run.revision);
    let job = app
        .workbook
        .get(id)
        .unwrap()
        .stats_job(run.columns.clone())
        .unwrap();
    let result = job.run(&mut |_| true).map_err(|e| e.to_string());
    let _ = app.update(Message::StatsDone(id, revision, result));
}

#[test]
fn theme_follows_mode() {
    let mut app = app();
    assert_eq!(app.current_theme().name, theme::DEFAULT_LIGHT);
    let _ = app.update(Message::SystemMode(iced::theme::Mode::Dark));
    assert_eq!(app.current_theme().name, theme::DEFAULT_DARK);
    let _ = app.update(Message::Action(Action::Mode(ModePreference::Light)));
    assert_eq!(app.current_theme().name, theme::DEFAULT_LIGHT);
}

#[test]
fn missing_theme_falls_back_to_built_in() {
    let (themes, _) = theme::load_all(None);
    let mut config = Config::default();
    config.appearance.mode = ModePreference::Dark;
    config.appearance.dark_theme = "Gone".into();
    assert_eq!(pick_theme(&themes, &config, None).name, theme::DEFAULT_DARK);
}

#[test]
fn settings_dialog_saves_config() {
    let mut app = app();
    let _ = app.update(Message::Action(Action::Settings));
    assert!(matches!(app.dialog, Some(Dialog::Settings(_))));
    let _ = app.update(Message::Settings(SettingsMessage::Mode(
        ModePreference::Dark,
    )));
    let _ = app.update(Message::Settings(SettingsMessage::Save));
    assert!(app.dialog.is_none());
    assert_eq!(app.config.appearance.mode, ModePreference::Dark);

    let _ = app.update(Message::Action(Action::About));
    let _ = app.update(Message::CloseDialog);
    assert!(app.dialog.is_none());
}

#[test]
fn menu_sections_follow_the_target() {
    use veta_core::arrow::datatypes::{DataType, Field, Fields};
    let titles = |target, column_type: Option<&DataType>| -> Vec<&str> {
        panes::menu_sections(target, 1, 1, column_type, None)
            .into_iter()
            .map(|(title, _)| title)
            .collect()
    };
    let point = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int64, true)]));
    assert_eq!(titles(MenuTarget::Blank, None), ["ST_MENU_SECTION_ADD"]);
    assert_eq!(
        titles(MenuTarget::Rows, None),
        ["ST_MENU_SECTION_CLIPBOARD", "ST_MENU_SECTION_ROWS"]
    );
    assert_eq!(
        titles(MenuTarget::Cell, Some(&DataType::Int64)),
        [
            "ST_MENU_SECTION_CLIPBOARD",
            "ST_MENU_SECTION_CELL",
            "ST_MENU_SECTION_FILTER",
            "ST_MENU_SECTION_ROWS"
        ]
    );
    assert_eq!(
        titles(MenuTarget::Column(0), Some(&point)),
        [
            "ST_MENU_SECTION_CLIPBOARD",
            "ST_MENU_SECTION_COLUMN",
            "ST_MENU_SECTION_DATA"
        ]
    );
    // Adding inside a struct or list stays, in the Column or Cell section.
    let last = |target, column_type: &DataType| {
        let sections = panes::menu_sections(target, 1, 1, Some(column_type), None);
        sections[1].1.last().map(|(_, label, _, _)| label.clone())
    };
    let tags = DataType::new_list(DataType::Utf8, true);
    assert_eq!(
        last(MenuTarget::Column(0), &point).as_deref(),
        Some("Add field…")
    );
    assert_eq!(
        last(MenuTarget::Cell, &tags).as_deref(),
        Some("Add item at the end")
    );
}

/// The open step dialog, if it is a `D`.
fn step_dialog<D: 'static>(app: &App) -> Option<&D> {
    match &app.dialog {
        Some(Dialog::Step { dialog, .. }) => dialog.as_any().downcast_ref(),
        _ => None,
    }
}
