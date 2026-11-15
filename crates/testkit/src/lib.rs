//! Test-only helpers shared across the workspace.
//!
//! Holds the counting allocator used to assert peak memory under load, and
//! [`TempDir`], a directory a test owns. Used only as a dev-dependency.
//!
//! The allocator is the pinned `cap` crate's `Cap<System>` with its `stats`
//! feature, so this repository holds no `unsafe` of its own. Install it in a
//! test binary of its own (an integration test file with one test), because
//! the counters are process-wide and any other test running at the same time
//! would add to them:
//!
//! ```
//! use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};
//!
//! #[global_allocator]
//! static ALLOC: CountingAllocator = counting_allocator();
//!
//! let window = PeakWindow::start(&ALLOC);
//! let v = vec![0u8; 4096];
//! assert!(window.peak_bytes() >= 4096);
//! drop(v);
//! ```

mod peak;
mod temp;

pub use peak::{
    CountingAllocator, PeakExceeded, PeakWindow, assert_peak_below, counting_allocator,
};
pub use temp::TempDir;
