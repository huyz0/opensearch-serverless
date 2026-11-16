//! What deletes still owe: one intent per deleted index, written before its
//! descriptor is removed and taken once it falls due.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ReclaimQueue.java`.
//! An intent is filed under the minute its delay ends in, one immutable
//! object each, so nothing is rewritten. A delete that crashed after its
//! intent leaves an intent naming whatever is left; one whose delete then
//! lost finds its index live and is dropped. Safe to drain twice: every step
//! of a reclaim is conditional or scoped to a uuid no live index has.

use std::sync::Arc;

use oss_blob::{BlobStore, ByteRange, MAX_LIST_KEYS, SliceSource};
use oss_format::{
    MAX_RECLAIM_INTENT_BYTES, RECLAIM_PREFIX, ReclaimIntent, reclaim_bucket, reclaim_bucket_start,
    reclaim_intent_key,
};

use crate::error::MetaError;

/// How long after a delete its intent falls due: two minutes, far longer
/// than a lease TTL plus the incarnation fence, which is how long a writer
/// that missed the delete can go on acting (the Java's
/// `DEFAULT_DELAY_MILLIS`).
pub const DEFAULT_RECLAIM_DELAY_MILLIS: i64 = 120_000;

/// A bucket's length: an intent is due once its whole minute has passed.
const BUCKET_MILLIS: i64 = 60_000;

/// The reclaim queue of one deployment.
pub struct ReclaimQueue {
    store: Arc<dyn BlobStore>,
    base: String,
    prefix: String,
    delay_millis: i64,
}

impl ReclaimQueue {
    /// The queue under `base`, empty or ending in `/`, due after
    /// [`DEFAULT_RECLAIM_DELAY_MILLIS`].
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end
    /// in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        // Checks the base as every key under it will be.
        reclaim_intent_key(base, "b", "i", "u")?;
        Ok(Self {
            store,
            // ALLOC: the base and the queue's prefix, once a queue.
            base: base.to_owned(),
            prefix: [base, RECLAIM_PREFIX].concat(),
            delay_millis: DEFAULT_RECLAIM_DELAY_MILLIS,
        })
    }

    /// Sets how long after a delete its intent falls due; a negative delay
    /// is none.
    #[must_use]
    pub fn with_delay_millis(mut self, millis: i64) -> Self {
        self.delay_millis = millis.max(0);
        self
    }

    /// Records what a delete about to happen will owe: one PUT, filed under
    /// the minute `now_millis` plus the delay falls in. The delete must not
    /// proceed when this fails.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a name or uuid that makes no key, or a due
    /// time outside 1970 to 9999; [`MetaError::Blob`] when the PUT fails.
    pub fn enqueue(&self, intent: &ReclaimIntent<'_>, now_millis: i64) -> Result<(), MetaError> {
        let bucket = reclaim_bucket(now_millis.saturating_add(self.delay_millis))?;
        let bucket = std::str::from_utf8(&bucket).unwrap_or_default();
        let key = reclaim_intent_key(&self.base, bucket, &intent.name, &intent.uuid)?;
        let mut body = [0u8; MAX_RECLAIM_INTENT_BYTES];
        let len = intent.write(&mut body)?;
        let body = body.get(..len).unwrap_or_default();
        self.store
            .put(&key, &mut SliceSource::new(body), &mut [0u8; 256])?;
        Ok(())
    }

    /// Takes every intent due by `now_millis` in a bucket `owns_bucket`
    /// accepts, handing each to `reclaim`, and answers how many were taken.
    ///
    /// An intent is taken, its object deleted, once `reclaim` succeeds, or
    /// when its body is not an intent a queue wrote; one whose GET or
    /// `reclaim` fails is kept for the next drain. Buckets sort by time, so
    /// the walk stops at the first bucket not yet due: a drain with nothing
    /// due is one LIST, whatever the queue holds. A bucket this caller does
    /// not own costs no request when it ends inside a page, and one LIST to
    /// skip the rest of it when a page ends inside it.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the queue cannot be listed or a taken intent
    /// cannot be deleted.
    pub fn drain(
        &self,
        now_millis: i64,
        owns_bucket: &dyn Fn(&str) -> bool,
        reclaim: &mut dyn FnMut(&ReclaimIntent<'_>) -> Result<(), MetaError>,
    ) -> Result<usize, MetaError> {
        let mut taken = 0usize;
        // ALLOC: one LIST's keys, at most MAX_LIST_KEYS of MAX_KEY_BYTES.
        let mut keys: Vec<String> = Vec::with_capacity(MAX_LIST_KEYS);
        let mut after: Option<String> = None;
        let mut body = [0u8; MAX_RECLAIM_INTENT_BYTES];
        loop {
            keys.clear();
            // LIST: the reclaim queue, from its oldest bucket to the first
            // not yet due; one a drain, one more a thousand due intents, and
            // at most one more for a page that ends in another node's bucket.
            let page = self.store.list_prefix(
                &self.prefix,
                after.as_deref(),
                MAX_LIST_KEYS,
                &mut |key, _| {
                    // ALLOC: one key a listed intent, bounded by the page.
                    keys.push(key.to_owned());
                },
            )?;
            for key in &keys {
                let rest = key.get(self.prefix.len()..).unwrap_or_default();
                let Some((bucket, _)) = rest.split_once('/') else {
                    continue;
                };
                let Some(start) = reclaim_bucket_start(bucket) else {
                    continue;
                };
                if start.saturating_add(BUCKET_MILLIS) > now_millis {
                    return Ok(taken);
                }
                if !owns_bucket(bucket) {
                    continue;
                }
                if self.take(key, &mut body, reclaim) {
                    self.store.delete(key)?;
                    taken = taken.saturating_add(1);
                }
            }
            let Some(last) = keys.pop() else {
                return Ok(taken);
            };
            if !page.truncated {
                return Ok(taken);
            }
            after = Some(self.resume_after(last, owns_bucket));
        }
    }

    /// Where the next page starts after `last`: past the rest of its bucket
    /// when that bucket is another node's, so a large one costs one more
    /// LIST rather than one a thousand of its keys. Buckets are twelve
    /// digits and `'0'` sorts after `'/'`, so `<bucket>0` follows every key
    /// of the bucket and precedes every later bucket.
    fn resume_after(&self, last: String, owns_bucket: &dyn Fn(&str) -> bool) -> String {
        let rest = last.get(self.prefix.len()..).unwrap_or_default();
        match rest.split_once('/') {
            Some((bucket, _)) if reclaim_bucket_start(bucket).is_some() && !owns_bucket(bucket) => {
                // ALLOC: one cursor a page, a key long.
                [self.prefix.as_str(), bucket, "0"].concat()
            }
            _ => last,
        }
    }

    /// Reads and reclaims one intent: whether it is finished.
    fn take(
        &self,
        key: &str,
        body: &mut [u8; MAX_RECLAIM_INTENT_BYTES],
        reclaim: &mut dyn FnMut(&ReclaimIntent<'_>) -> Result<(), MetaError>,
    ) -> bool {
        let mut len = 0usize;
        let mut too_long = false;
        let read = self.store.get(
            key,
            ByteRange::Full,
            &mut [0u8; 512],
            &mut |chunk: &[u8]| {
                let end = len.saturating_add(chunk.len());
                match body.get_mut(len..end) {
                    Some(slot) => {
                        slot.copy_from_slice(chunk);
                        len = end;
                    }
                    None => too_long = true,
                }
                Ok(())
            },
        );
        if read.is_err() {
            // A failed GET says nothing about the intent: kept. One taken
            // by another drain since the listing is gone, and no listing
            // returns it again.
            return false;
        }
        if too_long {
            // Longer than any intent a queue writes.
            return true;
        }
        match ReclaimIntent::parse(body.get(..len).unwrap_or_default()) {
            Ok(intent) => reclaim(&intent).is_ok(),
            // The whole body was read, and it is no intent this queue wrote.
            Err(_) => true,
        }
    }
}
