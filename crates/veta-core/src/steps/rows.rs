use std::ops::Range;

/// Maps an output row range of a delete step to the previous step's row
/// ranges that survive.
pub(super) fn kept_ranges(deleted: &[Range<usize>], output: Range<usize>) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    // Walk kept segments of the previous level, tracking their output offset.
    let mut prev_start = 0; // start of the current kept segment (previous level)
    let mut out_start = 0; // its position in the output
    let push = |segment: Range<usize>, out_offset: usize, result: &mut Vec<Range<usize>>| {
        let seg_out = out_offset..out_offset + segment.len();
        let from = output.start.max(seg_out.start);
        let to = output.end.min(seg_out.end);
        if from < to {
            let shift = segment.start as isize - seg_out.start as isize;
            result.push((from as isize + shift) as usize..(to as isize + shift) as usize);
        }
    };
    for d in deleted {
        let segment = prev_start..d.start;
        if out_start >= output.end {
            return result;
        }
        push(segment.clone(), out_start, &mut result);
        out_start += segment.len();
        prev_start = d.end;
    }
    if out_start < output.end {
        // Last kept segment runs to the end of the previous level.
        push(
            prev_start..prev_start + (output.end - out_start),
            out_start,
            &mut result,
        );
    }
    result
}

/// Sorts and merges row ranges, dropping empty ones.
pub fn normalize_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.retain(|r| !r.is_empty());
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}
