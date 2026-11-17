//! Where a write's acknowledgement time goes: per caller, the wait for its
//! window to close, the wait behind the blob in flight, and its own blob's
//! write; per blob, recording an epoch, checking shards new to it, and the
//! PUT. Sums since the last reset, read as means.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::log::window::WindowResult;

/// The sums, each added to without a lock.
#[derive(Debug, Default)]
pub(crate) struct CommitTimings {
    callers: AtomicU64,
    window_nanos: AtomicU64,
    behind_nanos: AtomicU64,
    write_nanos: AtomicU64,
    pub(crate) epoch_nanos: AtomicU64,
    pub(crate) admission_nanos: AtomicU64,
    pub(crate) admission_checks: AtomicU64,
    pub(crate) put_nanos: AtomicU64,
    pub(crate) puts: AtomicU64,
    pub(crate) epochs_opened: AtomicU64,
}

/// Adds `took` to a sum of nanoseconds, saturating.
pub(crate) fn add_nanos(sum: &AtomicU64, took: Duration) {
    sum.fetch_add(
        u64::try_from(took.as_nanos()).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
}

impl CommitTimings {
    /// Counts one caller that joined at `joined` and whose window was
    /// written as `result` records. A window never written counts nothing.
    pub(crate) fn caller(&self, joined: Instant, result: &WindowResult) {
        let (Some(started), Some(done)) = (result.flush_started, result.done) else {
            return;
        };
        let waited = started.saturating_duration_since(joined);
        let for_window = result.closes_at.map_or(waited, |closes| {
            waited.min(closes.saturating_duration_since(joined))
        });
        self.callers.fetch_add(1, Ordering::Relaxed);
        add_nanos(&self.window_nanos, for_window);
        add_nanos(&self.behind_nanos, waited.saturating_sub(for_window));
        add_nanos(&self.write_nanos, done.saturating_duration_since(started));
    }

    /// The sums as they stand, with the hedge counts kept beside them.
    pub(crate) fn snapshot(&self, hedges: u64, hedges_won: u64) -> AckTimings {
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        AckTimings {
            callers: n(&self.callers),
            window_nanos: n(&self.window_nanos),
            behind_nanos: n(&self.behind_nanos),
            write_nanos: n(&self.write_nanos),
            puts: n(&self.puts),
            epoch_nanos: n(&self.epoch_nanos),
            admission_nanos: n(&self.admission_nanos),
            admission_checks: n(&self.admission_checks),
            put_nanos: n(&self.put_nanos),
            hedges,
            hedges_won,
            epochs_opened: n(&self.epochs_opened),
        }
    }

    /// Starts every sum over.
    pub(crate) fn reset(&self) {
        for sum in [
            &self.callers,
            &self.window_nanos,
            &self.behind_nanos,
            &self.write_nanos,
            &self.epoch_nanos,
            &self.admission_nanos,
            &self.admission_checks,
            &self.put_nanos,
            &self.puts,
            &self.epochs_opened,
        ] {
            sum.store(0, Ordering::Relaxed);
        }
    }
}

/// The committer's timings since the last reset (the Java's
/// `NodeLogCommitTimings`); its `Display` is the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AckTimings {
    /// Callers whose window was written.
    pub callers: u64,
    /// Their summed wait for their window to close.
    pub window_nanos: u64,
    /// Their summed wait behind the blob in flight before theirs.
    pub behind_nanos: u64,
    /// Their summed wait for their own blob's write.
    pub write_nanos: u64,
    /// Blob PUTs tried, hedges aside.
    pub puts: u64,
    /// Summed time recording epochs (or finding one open), per blob.
    pub epoch_nanos: u64,
    /// Summed time checking shards new to the epoch, per blob.
    pub admission_nanos: u64,
    /// Shards checked.
    pub admission_checks: u64,
    /// Summed time in the PUTs.
    pub put_nanos: u64,
    /// PUTs sent a second time.
    pub hedges: u64,
    /// Of those, the ones the second answered first.
    pub hedges_won: u64,
    /// Epochs opened.
    pub epochs_opened: u64,
}

/// A mean in milliseconds; zero for no samples.
#[expect(clippy::cast_precision_loss, reason = "a mean for a log line")]
fn mean_ms(nanos: u64, count: u64) -> f64 {
    if count == 0 {
        0.0
    } else {
        nanos as f64 / 1e6 / count as f64
    }
}

impl AckTimings {
    /// The mean wait of a caller for its window to close, in ms.
    #[must_use]
    pub fn window_ms(&self) -> f64 {
        mean_ms(self.window_nanos, self.callers)
    }

    /// The mean wait of a caller behind the blob before its own, in ms.
    #[must_use]
    pub fn behind_ms(&self) -> f64 {
        mean_ms(self.behind_nanos, self.callers)
    }

    /// The mean wait of a caller for its own blob's write, in ms.
    #[must_use]
    pub fn write_ms(&self) -> f64 {
        mean_ms(self.write_nanos, self.callers)
    }

    /// The mean PUT, in ms.
    #[must_use]
    pub fn put_ms(&self) -> f64 {
        mean_ms(self.put_nanos, self.puts)
    }
}

impl fmt::Display for AckTimings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "callers={} window={:.1} behind={:.1} write={:.1} | blobs={} epoch={:.1} \
             admission={:.1} ({} head reads) put={:.1} ms | hedged={} won={} | epochs opened={}",
            self.callers,
            self.window_ms(),
            self.behind_ms(),
            self.write_ms(),
            self.puts,
            mean_ms(self.epoch_nanos, self.puts),
            mean_ms(self.admission_nanos, self.puts),
            self.admission_checks,
            self.put_ms(),
            self.hedges,
            self.hedges_won,
            self.epochs_opened,
        )
    }
}
