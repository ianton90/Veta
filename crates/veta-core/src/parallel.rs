//! Running independent pieces of work on all cores.

use std::sync::atomic::{AtomicUsize, Ordering};

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
}
