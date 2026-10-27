//! [`Progress`]: how far the work on a frame has got, shared between the
//! core, which writes it, and the caller, which reads it (or cancels the
//! work) whenever it wants.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use crate::error::{Error, Result};

/// What the work is doing. Front ends word it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Task {
    Reading = 1,
    Writing,
    Computing,
}

/// How far the work on a frame has got. Clones share the same progress.
///
/// ```no_run
/// # fn main() -> veta_core::Result<()> {
/// let progress = veta_core::Progress::new();
/// let df = veta_core::read_parquet("big.parquet", false)?.with_progress(&progress);
/// // On another thread: progress.fraction(), progress.task(), progress.cancel().
/// df.write_parquet("copy.parquet", &df.writer_settings())?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<State>);

#[derive(Debug, Default)]
struct State {
    task: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    cancelled: AtomicBool,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// From 0 to 1, for the task under way.
    pub fn fraction(&self) -> f32 {
        let total = self.0.total.load(Ordering::Relaxed);
        match total {
            0 => 0.0,
            total => (self.0.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32,
        }
    }

    /// The task under way, if any.
    pub fn task(&self) -> Option<Task> {
        match self.0.task.load(Ordering::Relaxed) {
            1 => Some(Task::Reading),
            2 => Some(Task::Writing),
            3 => Some(Task::Computing),
            _ => None,
        }
    }

    /// Asks the work to stop: it does at its next piece, with
    /// [`Error::Cancelled`]. Work started later is cancelled too, until
    /// [`Self::reset`].
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    /// Clears a cancel and the task.
    pub fn reset(&self) {
        self.0.cancelled.store(false, Ordering::Relaxed);
        self.0.task.store(0, Ordering::Relaxed);
        self.0.done.store(0, Ordering::Relaxed);
        self.0.total.store(0, Ordering::Relaxed);
    }

    /// Starts `task`, of `total` units (rows, bytes, …).
    pub(crate) fn start(&self, task: Task, total: u64) {
        self.0.done.store(0, Ordering::Relaxed);
        self.0.total.store(total, Ordering::Relaxed);
        self.0.task.store(task as u8, Ordering::Relaxed);
    }

    /// `n` more units done; an error if the work was cancelled.
    pub(crate) fn advance(&self, n: u64) -> Result<()> {
        self.0.done.fetch_add(n, Ordering::Relaxed);
        self.check()
    }

    /// Sets the fraction done (out of 1); `false` if the work was cancelled.
    pub(crate) fn set_fraction(&self, fraction: f32) -> bool {
        const SCALE: u64 = 1_000_000;
        self.0.total.store(SCALE, Ordering::Relaxed);
        self.0.done.store(
            (fraction.clamp(0.0, 1.0) as f64 * SCALE as f64) as u64,
            Ordering::Relaxed,
        );
        !self.is_cancelled()
    }

    /// An error if the work was cancelled.
    pub(crate) fn check(&self) -> Result<()> {
        match self.is_cancelled() {
            true => Err(Error::Cancelled),
            false => Ok(()),
        }
    }
}
