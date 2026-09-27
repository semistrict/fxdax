//! Checks of offset and length ranges.
//!
//! Ported from `zircon/kernel/include/kernel/range_check.h`.

/// Returns whether [offset, offset + len) lies within [0, max) without overflowing.
pub fn in_range(offset: u64, len: u64, max: u64) -> bool {
    // trim offset/len to the range
    let Some(end) = offset.checked_add(len) else {
        return false; // offset + len wrapped
    };
    // we started off the end of the range, or the end exceeds the range
    offset <= max && end <= max
}

/// Trims [offset, offset + len) to end at `trim_to_len`, returning the new length, which may be
/// zero if it precisely trims. Returns `None` if the range is out of range.
pub fn trim_range(offset: u64, len: u64, trim_to_len: u64) -> Option<u64> {
    // trim offset/len to the range
    let end = offset.checked_add(len)?; // offset + len wrapped
    // we started off the end of the range
    if offset > trim_to_len {
        return None;
    }
    // trim to the range
    if end > trim_to_len { Some(trim_to_len - offset) } else { Some(len) }
}
