//! Column statistics in the side pane.

use super::*;

impl App {
    /// Statistics messages.
    pub(crate) fn stats_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::StatsProgress(id, revision, progress) => {
                if let Some(run) = &mut self.stats.running
                    && run.document == id
                    && run.revision == revision
                {
                    run.progress = progress;
                }
            }
            Message::StatsDone(id, revision, result) => {
                if self
                    .stats
                    .running
                    .as_ref()
                    .is_some_and(|r| r.document == id && r.revision == revision)
                {
                    self.stats.running = None;
                }
                let current = self.workbook.get(id).map(Document::revision);
                // Keep only results for the document's current data.
                self.stats
                    .cache
                    .retain(|(d, r, _), _| *d != id || Some(*r) == current);
                match result {
                    Ok(stats) if current == Some(revision) => {
                        for s in stats {
                            self.stats.cache.insert((id, revision, s.column.clone()), s);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self
                        .errors
                        .push(format!("Could not compute statistics: {e}")),
                }
            }
            _ => unreachable!("not a stats message"),
        }
        Task::none()
    }

    /// Columns the statistics pane shows: the selected ones.
    fn stats_columns(&self, id: DocumentId) -> Vec<String> {
        self.selected_column_names(id)
    }

    /// Starts computing statistics the pane needs and doesn't have, and
    /// stops a pass that is no longer needed.
    pub(crate) fn schedule_stats(&mut self) -> Task<Message> {
        let visible = self.show_side_pane && self.side_tab == SideTab::Stats;
        let wanted = self
            .active
            .filter(|_| visible)
            .and_then(|id| {
                let doc = self.workbook.get(id)?;
                (doc.status() == &Status::Ready).then(|| (id, doc.revision()))
            })
            .map(|(id, revision)| {
                let missing: Vec<String> = self
                    .stats_columns(id)
                    .into_iter()
                    .filter(|c| !self.stats.cache.contains_key(&(id, revision, c.clone())))
                    .collect();
                (id, revision, missing)
            })
            .filter(|(_, _, missing)| !missing.is_empty());
        if let Some(run) = &self.stats.running {
            let still_needed = wanted.as_ref().is_some_and(|(id, revision, missing)| {
                run.document == *id
                    && run.revision == *revision
                    && missing.iter().all(|c| run.columns.contains(c))
            });
            if still_needed {
                return Task::none();
            }
            run.cancel.store(true, Ordering::Relaxed);
            self.stats.running = None;
        }
        let Some((id, revision, columns)) = wanted else {
            return Task::none();
        };
        let Some(job) = self
            .workbook
            .get(id)
            .and_then(|d| d.stats_job(columns.clone()).ok())
        else {
            return Task::none();
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.stats.running = Some(StatsRun {
            document: id,
            revision,
            columns,
            progress: 0.0,
            cancel: cancel.clone(),
        });
        let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let mut reported = 0.0;
            let result = job.run(&mut |progress| {
                if progress - reported >= 0.01 {
                    reported = progress;
                    let _ = sender.unbounded_send(Message::StatsProgress(id, revision, progress));
                }
                !cancel.load(Ordering::Relaxed)
            });
            if !cancel.load(Ordering::Relaxed) {
                let result = result.map_err(|e| e.to_string());
                let _ = sender.unbounded_send(Message::StatsDone(id, revision, result));
            }
        });
        Task::run(receiver, |message| message)
    }

    /// What the statistics pane shows for document `id`.
    pub(crate) fn stats_view(&self, id: DocumentId) -> panes::StatsView {
        let Some(doc) = self.workbook.get(id) else {
            return panes::StatsView::default();
        };
        let revision = doc.revision();
        let columns = self
            .stats_columns(id)
            .into_iter()
            .map(|c| {
                let full = self.stats.cache.get(&(id, revision, c.clone())).cloned();
                match full {
                    Some(stats) => (c, Some(stats), false),
                    None => {
                        let quick = doc.quick_stats(&c);
                        (c, quick, true)
                    }
                }
            })
            .collect();
        let progress = self
            .stats
            .running
            .as_ref()
            .filter(|r| r.document == id && r.revision == revision)
            .map(|r| r.progress);
        let waiting = match doc.status() {
            Status::Ready => None,
            Status::Computing { .. } => Some("Waiting for the steps to be computed…"),
            Status::Broken { .. } => Some("A step has an error; fix it to see statistics."),
        };
        panes::StatsView {
            columns,
            progress,
            waiting,
        }
    }
}
