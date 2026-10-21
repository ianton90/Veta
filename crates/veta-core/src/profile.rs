//! Timings of the slow parts of full passes and saves, for measuring Veta
//! on other machines: when the `VETA_PROFILE` environment variable is set,
//! time spent in each named part is added up and [`timings`] returns it.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Each part's total time and how many times it ran.
type Totals = Mutex<BTreeMap<&'static str, (Duration, u64)>>;

fn totals() -> Option<&'static Totals> {
    static TOTALS: OnceLock<Option<Totals>> = OnceLock::new();
    TOTALS
        .get_or_init(|| std::env::var_os("VETA_PROFILE").map(|_| Mutex::new(BTreeMap::new())))
        .as_ref()
}

/// Whether timings are being collected.
pub fn enabled() -> bool {
    totals().is_some()
}

/// Adds `elapsed` to the part `name`.
pub fn add(name: &'static str, elapsed: Duration) {
    if let Some(totals) = totals()
        && let Ok(mut totals) = totals.lock()
    {
        let entry = totals.entry(name).or_default();
        entry.0 += elapsed;
        entry.1 += 1;
    }
}

/// Runs `f`, adding its time to the part `name`.
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    add(name, start.elapsed());
    result
}

/// One part's timing: its name, total time and how many times it ran.
pub type Timing = (&'static str, Duration, u64);

/// The collected timings by part name, or `None` when not collecting. Parts
/// done on several threads add up the time of all of them.
pub fn timings() -> Option<Vec<Timing>> {
    let totals = totals()?.lock().ok()?;
    Some(
        totals
            .iter()
            .map(|(name, (elapsed, count))| (*name, *elapsed, *count))
            .collect(),
    )
}

/// Threads the parts were spread over (to read the timings against).
pub fn threads() -> usize {
    crate::parallel::threads()
}
