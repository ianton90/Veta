//! Timings of the slow parts of full passes and saves, for measuring Veta
//! on other machines: when the `VETA_PROFILE` environment variable is set,
//! time spent in each named part is added up and [`report`] prints it.

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

/// The collected timings, one line per part, or `None` when not collecting.
/// Parts done on several threads add up the time of all of them.
pub fn report() -> Option<String> {
    let totals = totals()?.lock().ok()?;
    let mut text = format!("VETA_PROFILE ({} threads)\n", crate::parallel::threads());
    for (name, (elapsed, count)) in totals.iter() {
        text.push_str(&format!(
            "  {name:<40} {:>9.2} s  ({count} times)\n",
            elapsed.as_secs_f64()
        ));
    }
    Some(text)
}
