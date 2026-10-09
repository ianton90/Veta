//! Veta's desktop interface, built with iced.
//!
//! The iced state holds the core [`Workbook`] (the model) plus GUI-only state
//! (tabs, grid scroll positions, panes). Messages that change data will be
//! turned into core commands; see `docs/ARCHITECTURE.md`.

mod grid;
mod panes;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iced::widget::{column, container, row, rule};
use iced::{Element, Length, Size, Subscription, Task, event, keyboard, window};
use veta_core::{Document, DocumentId, OpenOptions, Workbook};

use crate::grid::{GridEvent, GridView};

/// Opens the main window with `files` and blocks until it is closed.
pub fn run(files: Vec<PathBuf>) -> iced::Result {
    iced::application(move || App::new(files.clone()), App::update, App::view)
        .title(App::title)
        .subscription(App::subscription)
        .window_size(Size::new(1280.0, 800.0))
        .run()
}

#[derive(Debug, Default)]
struct App {
    workbook: Workbook,
    /// GUI state per open document, in tab order.
    tabs: Vec<Tab>,
    active: Option<DocumentId>,
    show_side_pane: bool,
    /// Files currently being opened in the background.
    opening: Vec<PathBuf>,
    errors: Vec<String>,
}

#[derive(Debug)]
struct Tab {
    id: DocumentId,
    grid: GridView,
}

#[derive(Debug, Clone)]
enum Message {
    OpenDialog,
    FilesPicked(Vec<PathBuf>),
    Opened(PathBuf, Result<Loaded, String>),
    SelectTab(DocumentId),
    CloseTab(DocumentId),
    CloseActiveTab,
    Grid(DocumentId, GridEvent),
    ToggleSidePane,
    DismissError(usize),
}

/// A document opened on a worker thread. Messages must be `Clone`, documents
/// are not, so the document travels in a take-once slot.
#[derive(Debug, Clone)]
struct Loaded(Arc<Mutex<Option<Document>>>);

impl Loaded {
    fn new(document: Document) -> Self {
        Self(Arc::new(Mutex::new(Some(document))))
    }

    fn take(&self) -> Option<Document> {
        self.0.lock().ok()?.take()
    }
}

impl App {
    fn new(files: Vec<PathBuf>) -> (Self, Task<Message>) {
        let mut app = Self {
            show_side_pane: true,
            ..Self::default()
        };
        let task = app.open(files);
        (app, task)
    }

    fn title(&self) -> String {
        match self.active_document() {
            Some((_, doc)) => format!("{} — Veta", doc.title()),
            None => "Veta".to_owned(),
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        event::listen_with(|event, status, _window| match event {
            iced::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::FilesPicked(vec![path]))
            }
            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if modifiers.command() && status == event::Status::Ignored =>
            {
                match key.as_ref() {
                    keyboard::Key::Character("o") => Some(Message::OpenDialog),
                    keyboard::Key::Character("w") => Some(Message::CloseActiveTab),
                    _ => None,
                }
            }
            _ => None,
        })
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::OpenDialog => {
                return Task::perform(pick_files(), |files| {
                    Message::FilesPicked(files.unwrap_or_default())
                });
            }
            Message::FilesPicked(files) => return self.open(files),
            Message::Opened(path, result) => {
                self.opening.retain(|p| p != &path);
                match result.map(|loaded| loaded.take()) {
                    Ok(Some(document)) => {
                        let grid = GridView::new(&document);
                        let id = self.workbook.add(document);
                        self.tabs.push(Tab { id, grid });
                        self.active = Some(id);
                    }
                    Ok(None) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not open {}: {e}", path.display())),
                }
            }
            Message::SelectTab(id) => self.active = Some(id),
            Message::CloseTab(id) => self.close(id),
            Message::CloseActiveTab => {
                if let Some(id) = self.active {
                    self.close(id);
                }
            }
            Message::Grid(id, event) => {
                if let (Some(doc), Some(tab)) = (
                    self.workbook.get(id),
                    self.tabs.iter_mut().find(|t| t.id == id),
                ) {
                    tab.grid.apply(event, doc);
                }
            }
            Message::ToggleSidePane => self.show_side_pane = !self.show_side_pane,
            Message::DismissError(index) => {
                if index < self.errors.len() {
                    self.errors.remove(index);
                }
            }
        }
        Task::none()
    }

    /// Starts opening `files` in the background. Files already open are
    /// focused instead.
    fn open(&mut self, files: Vec<PathBuf>) -> Task<Message> {
        let mut tasks = Vec::new();
        for path in files {
            if let Some(id) = self.find_open(&path) {
                self.active = Some(id);
                continue;
            }
            if self.opening.contains(&path) {
                continue;
            }
            self.opening.push(path.clone());
            let worker_path = path.clone();
            tasks.push(Task::perform(
                blocking(move || {
                    Document::open(worker_path, OpenOptions::default())
                        .map(Loaded::new)
                        .map_err(|e| e.to_string())
                }),
                move |result| Message::Opened(path.clone(), result),
            ));
        }
        Task::batch(tasks)
    }

    fn find_open(&self, path: &Path) -> Option<DocumentId> {
        let target = path.canonicalize().ok()?;
        self.workbook
            .iter()
            .find(|(_, doc)| {
                doc.path().and_then(|p| p.canonicalize().ok()).as_ref() == Some(&target)
            })
            .map(|(id, _)| id)
    }

    fn close(&mut self, id: DocumentId) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return;
        };
        self.tabs.remove(index);
        self.workbook.close(id);
        if self.active == Some(id) {
            // Focus the tab that took its place, or the new last one.
            self.active = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|t| t.id);
        }
    }

    fn active_document(&self) -> Option<(&Tab, &Document)> {
        let id = self.active?;
        let tab = self.tabs.iter().find(|t| t.id == id)?;
        Some((tab, self.workbook.get(id)?))
    }

    fn view(&self) -> Element<'_, Message> {
        let main: Element<'_, Message> = match self.active_document() {
            Some((tab, doc)) => {
                let id = tab.id;
                let grid = grid::grid(&tab.grid, move |e| Message::Grid(id, e));
                if self.show_side_pane {
                    row![
                        container(grid).width(Length::Fill).height(Length::Fill),
                        rule::vertical(1),
                        panes::side_pane(doc),
                    ]
                    .into()
                } else {
                    container(grid)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                }
            }
            None => panes::empty_state(!self.opening.is_empty()),
        };

        column![
            panes::ribbon(self.active.is_some(), self.show_side_pane),
            rule::horizontal(1),
            panes::errors(&self.errors),
            container(main).height(Length::Fill),
            rule::horizontal(1),
            panes::tab_bar(
                self.tabs
                    .iter()
                    .filter_map(|t| Some((t.id, self.workbook.get(t.id)?.title()))),
                self.active
            ),
            rule::horizontal(1),
            panes::status_bar(self.active_document(), self.opening.len()),
        ]
        .into()
    }
}

async fn pick_files() -> Option<Vec<PathBuf>> {
    let handles = rfd::AsyncFileDialog::new()
        .set_title("Open Parquet files")
        .add_filter("Parquet", &["parquet", "parq", "pq"])
        .add_filter("All files", &["*"])
        .pick_files()
        .await?;
    Some(handles.iter().map(|h| h.path().to_path_buf()).collect())
}

/// Runs blocking work on its own thread so the UI stays responsive.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(work());
    });
    receiver.await.expect("worker thread panicked")
}

#[cfg(test)]
mod tests {
    use super::*;
    use veta_testkit::{TempDir, fixtures};

    fn app_with(paths: &[&Path]) -> App {
        let mut app = App::default();
        for path in paths {
            let doc = Document::open(path, OpenOptions::default()).unwrap();
            let _ = app.update(Message::Opened(path.to_path_buf(), Ok(Loaded::new(doc))));
        }
        app
    }

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
        let _ = app.update(Message::CloseActiveTab);
        assert_eq!(app.active, Some(ids[2]));
        let _ = app.update(Message::CloseTab(ids[2]));
        assert_eq!(app.active, Some(ids[0]));
        let _ = app.update(Message::CloseTab(ids[0]));
        assert_eq!(app.active, None);
        assert!(app.workbook.is_empty());
    }

    #[test]
    fn open_failure_is_reported() {
        let mut app = App::default();
        app.opening.push("x.parquet".into());
        let _ = app.update(Message::Opened("x.parquet".into(), Err("boom".into())));
        assert!(app.opening.is_empty());
        assert_eq!(app.errors.len(), 1);
        let _ = app.update(Message::DismissError(0));
        assert!(app.errors.is_empty());
    }
}
