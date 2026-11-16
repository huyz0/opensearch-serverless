//! Joining records to windows: the youngest that takes them, splitting
//! across new ones where they would carry one past its bounds.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use oss_format::{ShardKey, WalRecord};

use super::{CommitError, Inner, NodeLogCommitter};
use crate::log::window::{Counted, Latch, Window};

impl NodeLogCommitter {
    /// Adds a shard's records to the youngest window, splitting them across
    /// new windows, in order, where they would carry one past its bounds.
    /// Returns the windows they joined, oldest first.
    pub(super) fn join_all(
        &self,
        inner: &mut Inner,
        shard: &ShardKey,
        term: i64,
        records: Vec<Counted>,
    ) -> Vec<Arc<Latch>> {
        let (max_bytes, max_records) = (self.config.max_bytes, self.config.max_records);
        // ALLOC: one entry a window joined, usually one.
        let mut joined: Vec<Arc<Latch>> = Vec::with_capacity(1);
        let mut rest = records.into_iter().peekable();
        while rest.peek().is_some() {
            let mut last = inner
                .windows
                .back_mut()
                .filter(|w| !w.full(max_bytes, max_records));
            let (room_bytes, room_records) = last.as_ref().map_or((max_bytes, max_records), |w| {
                (
                    max_bytes.saturating_sub(w.bytes),
                    max_records.saturating_sub(w.records),
                )
            });
            // ALLOC: the records joining one window, moved in, bounded by its record limit.
            let (mut bytes, mut taken) = (0u64, Vec::new());
            while let Some(next) = rest.peek() {
                if u64::try_from(taken.len()).unwrap_or(u64::MAX) >= room_records {
                    break;
                }
                if !taken.is_empty() && bytes.saturating_add(next.content) > room_bytes {
                    break;
                }
                if taken.is_empty()
                    && next.content > room_bytes
                    && let Some(w) = last.as_deref_mut()
                {
                    // Does not fit what is left of a window others joined
                    // (an open window always holds records): it starts the
                    // next one.
                    w.close_early = true;
                    break;
                }
                bytes = bytes.saturating_add(next.content);
                if let Some(record) = rest.next() {
                    // ALLOC: the records joining this window, moved, not copied.
                    taken.push(record);
                }
            }
            if taken.is_empty() {
                // The open window was closed for it; the next pass opens one.
                self.changed.notify_all();
                continue;
            }
            let Some(window) = self.join(inner, shard, term, taken) else {
                break;
            };
            if rest.peek().is_some() {
                // More of this shard's records follow: this window is as
                // full as it gets.
                window.close_early = true;
                self.changed.notify_all();
            }
            let latch = Arc::clone(&window.latch);
            if joined.last().is_none_or(|l| !Arc::ptr_eq(l, &latch)) {
                joined.push(latch);
            }
        }
        joined
    }

    /// Adds to the youngest window that takes them, or opens one.
    fn join<'i>(
        &self,
        inner: &'i mut Inner,
        shard: &ShardKey,
        term: i64,
        records: Vec<Counted>,
    ) -> Option<&'i mut Window> {
        let (max_bytes, max_records) = (self.config.max_bytes, self.config.max_records);
        if inner
            .windows
            .back()
            .is_none_or(|w| w.full(max_bytes, max_records))
        {
            let now = Instant::now();
            let mut closes_at = now.checked_add(self.config.window).unwrap_or(now);
            if self.idle_flush.load(Ordering::Relaxed) {
                // Paced by the last blob: a record finding the log quiet for
                // a whole window goes at once.
                let paced = inner
                    .last_flush_started
                    .and_then(|t| t.checked_add(self.config.window))
                    .map_or(now, |t| t.max(now));
                closes_at = closes_at.min(paced);
            }
            inner.windows.push_back(Window::new(closes_at));
        }
        let window = inner.windows.back_mut()?;
        window.add(shard, term, records);
        if window.full(max_bytes, max_records) {
            // Whoever lingers on it can write it now.
            self.changed.notify_all();
        }
        Some(window)
    }
}

/// Encodes each record once, with the bytes of id and source a window
/// counts for it (the Java's `contentBytes`).
pub(super) fn encode_all(records: &[WalRecord<'_>]) -> Result<Vec<Counted>, CommitError> {
    records
        .iter()
        .map(|r| {
            Ok(Counted {
                record: r.encode().map_err(CommitError::Encode)?,
                content: u64::try_from(r.id().len().saturating_add(r.source().len()))
                    .unwrap_or(u64::MAX),
            })
        })
        // ALLOC: one encoded record a record given, which the caller holds already.
        .collect()
}
