//! A document's calls to pinned analyzers, held to the analyzer's budget.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use oss_ext::analyzer::{Analyze, Token};
use oss_ext::{CallError, Status};

/// One document's calls to a pinned analyzer, refused once they have taken
/// the analyzer's item budget (ten deadlines) between them.
pub(super) struct Metered<'a> {
    inner: &'a dyn Analyze,
    /// Nanoseconds the document's calls have taken so far, across its fields.
    spent: &'a AtomicU64,
}

impl<'a> Metered<'a> {
    pub(super) fn new(inner: &'a dyn Analyze, spent: &'a AtomicU64) -> Self {
        Self { inner, spent }
    }
}

impl Analyze for Metered<'_> {
    fn analyze(&self, text: &str) -> Result<Vec<Token>, CallError> {
        let Some(cap) = self.inner.item_budget() else {
            return self.inner.analyze(text);
        };
        let used = Duration::from_nanos(self.spent.load(Ordering::Relaxed));
        if used > cap {
            return Err(CallError::from_status(
                Status::Limit,
                b"the document's calls took more than ten deadlines",
            ));
        }
        let start = Instant::now();
        let out = self.inner.analyze(text);
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.spent.fetch_add(nanos, Ordering::Relaxed);
        out
    }

    fn item_budget(&self) -> Option<Duration> {
        self.inner.item_budget()
    }
}
