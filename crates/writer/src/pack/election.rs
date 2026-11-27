//! Which node runs a collection window's pass.
//!
//! The window is `floor(now / 10 minutes)` of the store's clock and a pass
//! runs every window; the collector is chosen once for a term of
//! [`TERM_WINDOWS`] windows (a day) by rendezvous hashing over the live
//! membership, so one member runs every pass of a term and keeps its
//! schedules warm. There is no lease: a node that disagrees about membership
//! also collects, which costs duplicate requests and a lost
//! compare-and-swap, never a wrong deletion.

/// How long a collection window lasts.
pub const WINDOW_MILLIS: u64 = 600_000;

/// The window number of a time in milliseconds.
pub const fn window_of(now_millis: u64) -> u64 {
    match now_millis.checked_div(WINDOW_MILLIS) {
        Some(window) => window,
        None => 0,
    }
}

/// How many windows one collector keeps the pass: a day. The collector's
/// schedules (when a pack is next asked about, the first of its two
/// sightings) live in its memory, so a collector that changed every window
/// would start cold each time: a pack would wait for the same member to be
/// chosen twice, and a pack that is named would be asked about every window.
pub const TERM_WINDOWS: u64 = 144;

/// The term a window belongs to.
pub const fn term_of(window: u64) -> u64 {
    match window.checked_div(TERM_WINDOWS) {
        Some(term) => term,
        None => 0,
    }
}

/// FNV-1a (64 bits) of a term's big-endian bytes and then a member's bytes.
fn weight(term: u64, member: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in term.to_be_bytes().iter().chain(member.as_bytes()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The member that collects in `window`: the one of `members` with the
/// greatest weight for the window's term (rendezvous hashing, ties to the
/// smaller id), so it is the same member every window of the term and the
/// choice moves only when that member leaves or one with a greater weight
/// joins, never because another member came or went. `None` when there are
/// no members. The hash is FNV-1a, not Rust's hasher, which may change
/// between versions.
pub fn collector_for(window: u64, members: &[String]) -> Option<&str> {
    let term = term_of(window);
    members
        .iter()
        .map(|m| (weight(term, m), m.as_str()))
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(a.1)))
        .map(|(_, m)| m)
}
