//! Deletes: one key, one key at a version, or a batch of up to
//! [`MAX_DELETE_KEYS`] keys a request (S3's `DeleteObjects`).

use super::{Emulator, Objects, holds, lock};
use crate::error::BlobResult;
use crate::op::S3Op;
use crate::register::tagged::Precondition;
use crate::store::{MAX_DELETE_KEYS, check_key};

/// Removes `key` and what is kept beside it, giving its bytes back.
pub(super) fn remove(objects: &mut Objects, key: &str) {
    objects.writers.remove(key);
    objects.tags.remove(key);
    if let Some(old) = objects.map.remove(key) {
        let n = u64::try_from(old.len()).unwrap_or(u64::MAX);
        objects.stored = objects.stored.saturating_sub(n);
    }
}

impl Emulator {
    /// One DELETE.
    pub(super) fn delete_one(&self, key: &str) -> BlobResult<()> {
        check_key(key)?;
        self.attempt(S3Op::Delete, || {
            remove(&mut lock(&self.objects), key);
            Ok(())
        })
    }

    /// One batch delete for each [`MAX_DELETE_KEYS`] keys, in order, after
    /// checking every key.
    pub(crate) fn delete_batch(&self, keys: &[&str]) -> BlobResult<()> {
        for key in keys {
            check_key(key)?;
        }
        for batch in keys.chunks(MAX_DELETE_KEYS) {
            self.attempt(S3Op::Delete, || {
                let mut objects = lock(&self.objects);
                for key in batch {
                    remove(&mut objects, key);
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// A DELETE that removes `key` only if its ETag is `etag`: `false`, a
    /// failed attempt as S3's 412, when it is not or there is no object.
    pub(crate) fn delete_if(&self, key: &str, etag_wanted: &str) -> BlobResult<bool> {
        check_key(key)?;
        let deleted = self.attempt(S3Op::Delete, || {
            let mut objects = lock(&self.objects);
            if !holds(&objects, key, Precondition::Matches(etag_wanted)) {
                return Ok(false);
            }
            remove(&mut objects, key);
            Ok(true)
        })?;
        if !deleted {
            self.meter.record_error();
        }
        Ok(deleted)
    }
}
