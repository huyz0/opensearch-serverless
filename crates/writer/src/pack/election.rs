//! Which node runs a collection window's pass.
//!
//! The window is `floor(now / 10 minutes)` of the store's clock; its collector
//! is the member at `hash(window) mod n` of the live membership sorted by
//! node id. There is no lease: a node that disagrees about membership also
//! collects, which costs duplicate requests and a lost compare-and-swap,
//! never a wrong deletion.

/// How long a collection window lasts.
pub const WINDOW_MILLIS: u64 = 600_000;

/// The window number of a time in milliseconds.
pub const fn window_of(now_millis: u64) -> u64 {
    match now_millis.checked_div(WINDOW_MILLIS) {
        Some(window) => window,
        None => 0,
    }
}

/// The member that collects in `window`, from `members` sorted by id; `None`
/// when there are no members. The hash is FNV-1a (64 bits) of the window's
/// big-endian bytes, not Rust's hasher, which may change between versions.
pub fn collector_for(window: u64, members: &[String]) -> Option<&str> {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in window.to_be_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let at = hash.checked_rem(u64::try_from(members.len()).ok()?)?;
    members.get(usize::try_from(at).ok()?).map(String::as_str)
}
