//! Running independent pieces of work on all cores.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};

use crate::error::{Error, Result};

/// Number of threads worth running at once.
pub(crate) fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// `f(0)`, …, `f(n - 1)`, on up to [`threads`] threads that each take the
/// next index when they finish one (so pieces of different cost still
/// share the cores evenly). Results are in index order.
pub(crate) fn map<T: Send>(n: usize, f: impl Fn(usize) -> Result<T> + Sync) -> Result<Vec<T>> {
    let workers = threads().min(n);
    if workers <= 1 {
        return (0..n).map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let done: Vec<Vec<(usize, Result<T>)>> = std::thread::scope(|scope| {
        let work: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            return done;
                        }
                        done.push((i, f(i)));
                    }
                })
            })
            .collect();
        work.into_iter()
            .map(|w| w.join().unwrap_or_default())
            .collect()
    });
    let mut results: Vec<Option<Result<T>>> = (0..n).map(|_| None).collect();
    for (i, result) in done.into_iter().flatten() {
        results[i] = Some(result);
    }
    results
        .into_iter()
        .map(|r| r.unwrap_or_else(|| Err(Error::InvalidCommand("a worker thread failed".into()))))
        .collect()
}

/// `produce(0)`, …, `produce(n - 1)` computed on up to [`threads`] threads
/// and returned in order, with at most `ahead` items computed past the one
/// being used (which bounds memory). Workers stop when this is dropped.
pub(crate) fn ordered<T: Send + 'static>(
    n: usize,
    ahead: usize,
    produce: impl Fn(usize) -> Result<T> + Send + Sync + 'static,
) -> Ordered<T> {
    let (send, results) = channel();
    let gate = Arc::new(Gate {
        taken: Mutex::new(0),
        moved: Condvar::new(),
        stop: AtomicBool::new(false),
    });
    let next = Arc::new(AtomicUsize::new(0));
    let produce = Arc::new(produce);
    let ahead = ahead.max(1);
    for _ in 0..threads().min(n) {
        let (send, gate, next, produce) =
            (send.clone(), gate.clone(), next.clone(), produce.clone());
        std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n || !gate.wait_for(i, ahead) {
                    return;
                }
                if send.send((i, produce(i))).is_err() {
                    return;
                }
            }
        });
    }
    Ordered {
        results,
        early: BTreeMap::new(),
        next: 0,
        n,
        gate,
    }
}

/// Lets workers run at most `ahead` items past the last one taken.
struct Gate {
    taken: Mutex<usize>,
    moved: Condvar,
    stop: AtomicBool,
}

impl Gate {
    /// Waits until item `i` may be computed; `false` when stopped.
    fn wait_for(&self, i: usize, ahead: usize) -> bool {
        let mut taken = self.taken.lock().unwrap_or_else(|e| e.into_inner());
        while i >= *taken + ahead && !self.stop.load(Ordering::Relaxed) {
            taken = self.moved.wait(taken).unwrap_or_else(|e| e.into_inner());
        }
        !self.stop.load(Ordering::Relaxed)
    }

    fn take(&self, count: usize) {
        *self.taken.lock().unwrap_or_else(|e| e.into_inner()) = count;
        self.moved.notify_all();
    }
}

/// The results of [`ordered`], in order.
pub(crate) struct Ordered<T> {
    results: Receiver<(usize, Result<T>)>,
    early: BTreeMap<usize, Result<T>>,
    next: usize,
    n: usize,
    gate: Arc<Gate>,
}

impl<T> Iterator for Ordered<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Result<T>> {
        if self.next >= self.n {
            return None;
        }
        let result = loop {
            if let Some(result) = self.early.remove(&self.next) {
                break result;
            }
            match self.results.recv() {
                Ok((i, result)) => {
                    self.early.insert(i, result);
                }
                Err(_) => break Err(Error::InvalidCommand("a worker thread failed".into())),
            }
        };
        self.next += 1;
        self.gate.take(self.next);
        Some(result)
    }
}

impl<T> Drop for Ordered<T> {
    fn drop(&mut self) {
        self.gate.stop.store(true, Ordering::Relaxed);
        self.gate.moved.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_their_order() {
        let squares = map(1000, |i| Ok(i * i)).unwrap();
        assert_eq!(squares, (0..1000).map(|i| i * i).collect::<Vec<_>>());
        assert!(map(0, Ok).unwrap().is_empty());
        let failed = map(10, |i| {
            if i == 7 {
                Err(Error::InvalidCommand("seven".into()))
            } else {
                Ok(i)
            }
        });
        assert!(failed.is_err());
    }

    #[test]
    fn ordered_results_come_in_order_and_stop_early() {
        let squares: Vec<usize> = ordered(500, 4, |i| {
            // Later items finish first sometimes.
            std::thread::sleep(std::time::Duration::from_micros(((i * 37) % 11) as u64));
            Ok(i * i)
        })
        .collect::<Result<_>>()
        .unwrap();
        assert_eq!(squares, (0..500).map(|i| i * i).collect::<Vec<_>>());
        assert_eq!(ordered(0, 4, Ok).count(), 0);
        // Taking a few and dropping the rest stops the workers.
        let computed = Arc::new(AtomicUsize::new(0));
        let counter = computed.clone();
        let first: Vec<usize> = ordered(10_000, 2, move |i| {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(i)
        })
        .take(3)
        .collect::<Result<_>>()
        .unwrap();
        assert_eq!(first, [0, 1, 2]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            computed.load(Ordering::Relaxed) < 100,
            "workers wait for the reader"
        );
    }
}
