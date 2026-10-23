//! Recipes: saving a document's steps as JSON, and applying them.

use super::*;

impl App {
    /// Recipe messages.
    pub(crate) fn recipes_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SaveRecipe => {
                if let Some(id) = self.active {
                    let name = self
                        .workbook
                        .get(id)
                        .map(|d| {
                            let title = d.title_or(&tr("ST_DOC_UNTITLED"));
                            let stem = title.rsplit_once('.').map_or(title.as_str(), |(s, _)| s);
                            format!("{stem}-recipe.json")
                        })
                        .unwrap_or_default();
                    let save = files::save(
                        tr("ST_RIBBON_BTN_SAVE_RECIPE"),
                        FileKind::Recipe,
                        name,
                        None,
                    );
                    return Task::perform(save, move |path| Message::SaveRecipeTo(id, path));
                }
            }
            Message::SaveRecipeTo(id, Some(path)) => {
                if let Some(doc) = self.workbook.get(id) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.trim_end_matches(".json").trim_end_matches(".recipe"))
                        .unwrap_or_default()
                        .to_owned();
                    let info = veta_core::recipe::RecipeInfo {
                        name,
                        source_file: doc
                            .path()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned()),
                    };
                    let result = veta_core::recipe::to_json(doc.steps(), &info)
                        .map_err(|e| error_text(&e))
                        .and_then(|text| std::fs::write(&path, text).map_err(io_text));
                    if let Err(e) = result {
                        self.errors
                            .push(tr_with("ST_MSG_RECIPE_SAVE_FAILED", &[("error", &e)]));
                    }
                }
            }
            Message::LoadRecipe => {
                if let Some(id) = self.active {
                    if !self.editable(id) {
                        self.not_editable(id);
                        return Task::none();
                    }
                    let open =
                        files::open_one(tr("ST_RIBBON_BTN_LOAD_RECIPE"), vec![FileKind::Recipe]);
                    return Task::perform(open, move |path| Message::LoadRecipeFrom(id, path));
                }
            }
            Message::LoadRecipeFrom(id, Some(path)) => match std::fs::read_to_string(&path) {
                // With steps already there, ask whether to replace them.
                Ok(text) if self.workbook.get(id).is_some_and(|d| !d.steps().is_empty()) => {
                    self.dialog = Some(Dialog::ConfirmRecipe { document: id, text });
                }
                Ok(text) => self.apply_recipe(id, &text, false),
                Err(e) => self.errors.push(tr_with(
                    "ST_MSG_RECIPE_LOAD_FAILED",
                    &[("error", &io_text(e))],
                )),
            },
            Message::RecipeChoice(choice) => {
                if let Some(Dialog::ConfirmRecipe { document, text }) = self.dialog.take()
                    && let Some(replace) = choice
                {
                    self.apply_recipe(document, &text, replace);
                }
            }
            Message::SaveRecipeTo(_, None) | Message::LoadRecipeFrom(_, None) => {}
            _ => unreachable!("not a recipes message"),
        }
        Task::none()
    }

    /// Recipe ribbon actions.
    pub(crate) fn recipes_action(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::LoadRecipe => self.handle(Message::LoadRecipe),
            Action::SaveRecipe => self.handle(Message::SaveRecipe),
            _ => unreachable!("not a recipes action"),
        }
    }

    /// Adds a recipe's steps (JSON `text`) after the document's, or with
    /// `replace`, in place of them (the recipe then applies to the source).
    fn apply_recipe(&mut self, id: DocumentId, text: &str, replace: bool) {
        let Some(doc) = self.workbook.get(id) else {
            return;
        };
        let schema = if replace {
            doc.schema_at(0).unwrap_or_else(|| doc.schema())
        } else {
            doc.schema()
        };
        match veta_core::recipe::from_json(text, &schema) {
            Ok(steps) if steps.is_empty() => {
                self.errors.push(tr("ST_MSG_RECIPE_EMPTY"));
            }
            Ok(steps) => {
                let command = if replace {
                    Command::ReplaceSteps { steps }
                } else {
                    Command::AppendSteps { steps }
                };
                if self.execute(id, command) {
                    self.after_compute(id);
                }
            }
            Err(e) => self.errors.push(tr_with(
                "ST_MSG_RECIPE_LOAD_FAILED",
                &[("error", &error_text(&e))],
            )),
        }
    }
}
