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
    NATIVE_COMPILE.load(Ordering::Relaxed)
}
