//! Whether a node compiles Painless source itself
//! (`serverless.scripting.painless.native_compile`, off by default; see
//! docs/adr/0069).

use std::sync::atomic::{AtomicBool, Ordering};

static NATIVE_COMPILE: AtomicBool = AtomicBool::new(false);

/// Sets whether Painless source is compiled natively. Process-wide: set once
/// at start, before a search runs.
pub fn set_native_compile(on: bool) {
    NATIVE_COMPILE.store(on, Ordering::Relaxed);
}

/// Whether Painless source is compiled natively.
pub fn native_compile() -> bool {
    #[cfg(test)]
    if let Some(on) = THIS_THREAD.with(std::cell::Cell::get) {
        return on;
    }
    NATIVE_COMPILE.load(Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    static THIS_THREAD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Sets the switch for the calling test's thread alone, so a test needs no
/// lock against the others that run in parallel.
#[cfg(test)]
pub(crate) fn set_for_this_thread(on: bool) {
    THIS_THREAD.with(|c| c.set(Some(on)));
}
