//! The memory budget: how much memory Veta may use for data (files loaded
//! into memory, page caches). Half of the machine's RAM unless set with
//! [`set_budget`].

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::Result;

const MIB: usize = 1024 * 1024;
/// The smallest budget allowed.
pub const MIN_BUDGET: usize = 64 * MIB;
/// Used when the machine's RAM can't be read.
const FALLBACK: usize = 1024 * MIB;

/// The set budget in bytes; 0 for the default.
static BUDGET: AtomicUsize = AtomicUsize::new(0);

/// The machine's RAM in bytes (0 if it can't be read).
pub fn total_ram() -> u64 {
    static TOTAL: OnceLock<u64> = OnceLock::new();
    *TOTAL.get_or_init(|| {
        let mut system = sysinfo::System::new();
        system.refresh_memory_specifics(sysinfo::MemoryRefreshKind::nothing().with_ram());
        system.total_memory()
    })
}

/// The default budget: half of the machine's RAM.
pub fn default_budget() -> usize {
    match usize::try_from(total_ram() / 2) {
        Ok(0) => FALLBACK,
        Ok(half) => half.max(MIN_BUDGET),
        Err(_) => usize::MAX,
    }
}

/// The current budget in bytes.
pub fn budget() -> usize {
    match BUDGET.load(Ordering::Relaxed) {
        0 => default_budget(),
        set => set,
    }
}

/// Sets the budget in bytes (at least [`MIN_BUDGET`]); `None` goes back to
/// the default.
pub fn set_budget(bytes: Option<usize>) {
    BUDGET.store(bytes.map_or(0, |b| b.max(MIN_BUDGET)), Ordering::Relaxed);
}

/// Reads a budget written as bytes with a unit (`512M`, `4G`, `4GB`, `2.5G`)
/// or a share of RAM (`25%`).
pub fn parse_budget(text: &str) -> Result<usize> {
    let invalid = || crate::error::invalid!(MemorySize, value = text);
    let text = text.trim();
    if let Some(percent) = text.strip_suffix('%') {
        let share: f64 = percent.trim().parse().map_err(|_| invalid())?;
        if !(share > 0.0 && share <= 100.0) {
            return Err(invalid());
        }
        let ram = match total_ram() {
            0 => FALLBACK as f64 * 2.0,
            ram => ram as f64,
        };
        return Ok((ram * share / 100.0) as usize);
    }
    let upper = text.to_ascii_uppercase();
    let number = upper.trim_end_matches('B').trim_end_matches('I');
    let (digits, unit) = match number.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&number[..i], c),
        _ => (number, ' '),
    };
    let scale = match unit {
        ' ' => 1.0,
        'K' => 1024.0,
        'M' => MIB as f64,
        'G' => (1024 * MIB) as f64,
        'T' => (1024 * 1024 * MIB) as f64,
        _ => return Err(invalid()),
    };
    let value: f64 = digits.trim().parse().map_err(|_| invalid())?;
    if value.is_nan() || value <= 0.0 {
        return Err(invalid());
    }
    Ok((value * scale) as usize)
}

/// Bytes of cell edits in a row before they are grouped into a snapshot of
/// the edited columns; 0 for the default.
static CELL_EDITS: AtomicUsize = AtomicUsize::new(0);

/// The default for [`cell_edits_limit`]: 64 MiB.
pub const DEFAULT_CELL_EDITS: usize = 64 * MIB;

/// Bytes of cell edits in a row that are kept edit by edit; past it they
/// are grouped into a snapshot of the edited columns (undone as one).
pub fn cell_edits_limit() -> usize {
    match CELL_EDITS.load(Ordering::Relaxed) {
        0 => DEFAULT_CELL_EDITS,
        set => set,
    }
}

/// Sets [`cell_edits_limit`]; `None` goes back to the default.
pub fn set_cell_edits_limit(bytes: Option<usize>) {
    CELL_EDITS.store(bytes.map_or(0, |b| b.max(1)), Ordering::Relaxed);
}

/// Page cache of one paged file.
pub(crate) fn page_cache(budget: usize) -> usize {
    (budget / 4).max(MIN_BUDGET)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_shares() {
        assert_eq!(parse_budget("512M").unwrap(), 512 * MIB);
        assert_eq!(parse_budget("4G").unwrap(), 4096 * MIB);
        assert_eq!(parse_budget("4gb").unwrap(), 4096 * MIB);
        assert_eq!(parse_budget("2.5GiB").unwrap(), 2560 * MIB);
        assert_eq!(parse_budget("1048576").unwrap(), MIB);
        let quarter = parse_budget("25%").unwrap();
        let half = parse_budget("50%").unwrap();
        assert!(quarter > 0 && half > quarter);
        for bad in ["", "abc", "4X", "0", "-1G", "150%", "0%"] {
            assert!(parse_budget(bad).is_err(), "{bad:?}");
        }
        assert!(default_budget() >= MIN_BUDGET);
    }
}
