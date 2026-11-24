//! Ending a joining node's warm-up: ready once enough of the bytes named are
//! in, once the wait runs out, or once every peer has been copied from.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::lifecycle::Readiness;
use crate::warm_copy::{Pause, Progress};

/// When a warming node is let in.
#[derive(Clone, Copy, Debug)]
pub struct Join {
    /// The share of the bytes named, in percent, that must be in.
    pub ready_percent: u64,
    /// The longest the node is held out.
    pub max_wait: Duration,
    /// How often the progress is looked at.
    pub poll: Duration,
}

/// Waits for `join`'s condition, then ends `readiness`'s warming. Sets
/// `expired` when the wait ran out, so the copy stops with it. Returns whether
/// the wait ran out.
pub fn finish_when_warm(
    readiness: &Readiness,
    progress: &Progress,
    copy_done: &AtomicBool,
    join: &Join,
    elapsed: &dyn Fn() -> Duration,
    pause: &dyn Pause,
    expired: &AtomicBool,
) -> bool {
    // A percent of a byte count, as the progress compares it.
    #[allow(clippy::cast_precision_loss)]
    let fraction = join.ready_percent as f64 / 100.0;
    loop {
        let out_of_time = elapsed() >= join.max_wait;
        let warm = progress.named() > 0 && progress.reached(fraction);
        if out_of_time || warm || copy_done.load(Ordering::Acquire) {
            if out_of_time && !warm {
                expired.store(true, Ordering::Release);
            }
            readiness.finish_warming();
            return out_of_time && !warm;
        }
        pause.pause(join.poll);
    }
}

#[cfg(test)]
mod tests;
