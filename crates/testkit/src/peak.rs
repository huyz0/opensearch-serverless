//! A counting global allocator and peak-memory assertions over it.

use std::alloc::System;
use std::fmt;

/// The counting global allocator: the system allocator wrapped by `cap`,
/// which tracks bytes in use, the running maximum, and the total ever
/// allocated.
pub type CountingAllocator = cap::Cap<System>;

/// A counting allocator with no limit, for a `#[global_allocator]` static.
pub const fn counting_allocator() -> CountingAllocator {
    cap::Cap::new(System, usize::MAX)
}

/// Peak-memory readings taken from a start point.
///
/// The running maximum cannot be reset, so the peak is the process maximum
/// minus the bytes in use when the window opened. If the process peaked
/// higher before the window opened, the reading is too high, never too low:
/// an assertion over it can fail falsely but cannot pass falsely.
#[derive(Debug)]
pub struct PeakWindow<'a> {
    alloc: &'a CountingAllocator,
    baseline: usize,
    total_at_start: usize,
}

impl<'a> PeakWindow<'a> {
    /// Opens a window at the allocator's current reading.
    pub fn start(alloc: &'a CountingAllocator) -> Self {
        Self {
            alloc,
            baseline: alloc.allocated(),
            total_at_start: alloc.total_allocated(),
        }
    }

    /// Bytes in use when the window opened.
    pub fn baseline(&self) -> usize {
        self.baseline
    }

    /// The most bytes in use at once since the window opened, above the
    /// baseline (an upper bound; see the type's notes).
    pub fn peak_bytes(&self) -> usize {
        self.alloc.max_allocated().saturating_sub(self.baseline)
    }

    /// Bytes in use now above the baseline, or zero if fewer are in use.
    pub fn live_bytes(&self) -> usize {
        self.alloc.allocated().saturating_sub(self.baseline)
    }

    /// Bytes allocated since the window opened, counting memory already
    /// freed: zero means nothing called the allocator.
    pub fn allocated_bytes(&self) -> usize {
        self.alloc
            .total_allocated()
            .saturating_sub(self.total_at_start)
    }

    /// Fails when the peak is over `bound` bytes.
    pub fn check_below(&self, bound: usize) -> Result<usize, PeakExceeded> {
        let peak = self.peak_bytes();
        if peak > bound {
            Err(PeakExceeded { peak, bound })
        } else {
            Ok(peak)
        }
    }
}

/// The peak since a window opened was over its bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeakExceeded {
    /// The measured peak above the baseline.
    pub peak: usize,
    /// The bound it was held to.
    pub bound: usize,
}

impl fmt::Display for PeakExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "peak memory {} bytes is over the bound of {} bytes",
            self.peak, self.bound
        )
    }
}

impl std::error::Error for PeakExceeded {}

/// Runs `work` and panics if the peak memory above the starting point went
/// over `bound` bytes. Returns `work`'s result and the measured peak.
pub fn assert_peak_below<R>(
    alloc: &CountingAllocator,
    bound: usize,
    work: impl FnOnce() -> R,
) -> (R, usize) {
    let window = PeakWindow::start(alloc);
    let out = work();
    match window.check_below(bound) {
        Ok(peak) => (out, peak),
        Err(e) => panic!("{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_exceeded_message() {
        let e = PeakExceeded {
            peak: 11,
            bound: 10,
        };
        assert_eq!(
            e.to_string(),
            "peak memory 11 bytes is over the bound of 10 bytes"
        );
    }

    #[test]
    fn an_idle_allocator_reads_zero() {
        // Not installed as the global allocator, so nothing moves its counters.
        let alloc = counting_allocator();
        let w = PeakWindow::start(&alloc);
        assert_eq!(
            (
                w.baseline(),
                w.peak_bytes(),
                w.live_bytes(),
                w.allocated_bytes()
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(w.check_below(0), Ok(0));
        let ((), peak) = assert_peak_below(&alloc, 0, || ());
        assert_eq!(peak, 0);
    }
}
