//! The emulator's clock: the time a HEAD reports an object was written and
//! the time it was asked, which only [`Emulator::advance_clock`] moves, so a
//! test sets an object's age exactly.

use super::{Emulator, ObjectMeta, lock};
use crate::error::BlobResult;
use crate::op::S3Op;
use crate::store::check_key;

/// The store's clock, in milliseconds, when an object was written and now:
/// what a HEAD's `Last-Modified` and `Date` say.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamps {
    pub(crate) born: u64,
    pub(crate) now: u64,
}

/// What a HEAD finds: the object's metadata, its version, its writer token
/// if it has one, and the clock stamps.
pub(crate) type Headed = (ObjectMeta, u64, Option<String>, Stamps);

impl Emulator {
    /// Moves the store's clock on by `millis`: every object grows that much
    /// older. The clock never moves otherwise.
    pub fn advance_clock(&self, millis: u64) {
        let mut objects = lock(&self.objects);
        objects.clock = objects.clock.saturating_add(millis);
    }

    /// One HEAD's answer as an age in milliseconds.
    pub(super) fn age_of(&self, key: &str) -> BlobResult<Option<u64>> {
        check_key(key)?;
        self.attempt(S3Op::Head, || {
            let objects = lock(&self.objects);
            Ok(objects.map.contains_key(key).then(|| {
                let born = objects.born.get(key).copied().unwrap_or(objects.clock);
                objects.clock.saturating_sub(born)
            }))
        })
    }

    /// [`BlobStore::head`], with the object's version and its writer token
    /// if it has one.
    pub(crate) fn head_with_writer(&self, key: &str) -> BlobResult<Option<Headed>> {
        check_key(key)?;
        self.attempt(S3Op::Head, || {
            let objects = lock(&self.objects);
            Ok(objects.map.get(key).map(|o| {
                let meta = ObjectMeta {
                    len: u64::try_from(o.len()).unwrap_or(u64::MAX),
                };
                let tag = objects.tags.get(key).copied().unwrap_or(0);
                // ALLOC: one token, bounded by the header line it came in.
                let clock = objects.clock;
                let stamps = Stamps {
                    born: objects.born.get(key).copied().unwrap_or(clock),
                    now: clock,
                };
                (meta, tag, objects.writers.get(key).cloned(), stamps)
            }))
        })
    }
}
