//! Listing, expiring and reaping the points in time of a deployment.

use oss_blob::{MAX_LIST_KEYS, SwapOutcome};
use oss_format::{POINTS_IN_TIME_PREFIX, PointInTime, RELEASED, looks_like_an_id};

use super::{ATTEMPTS, Expiry, PointInTimeStore, State};
use crate::error::MetaError;

impl PointInTimeStore {
    /// Expires `id` if its deadline has passed. The claim is the swap to the
    /// tombstone, so a keep-alive acknowledged elsewhere moves the
    /// generation and forces a reread rather than losing its files.
    fn expire(&self, id: &str, now: u64, buf: &mut [u8]) -> Result<Expiry, MetaError> {
        let state_key = self.state_key(id)?;
        let inventory = self.inventory_key(id)?;
        for _ in 0..ATTEMPTS {
            match self.state(id, buf)? {
                None => {
                    return Ok(Expiry {
                        live: None,
                        removed: false,
                    });
                }
                Some(State::Released) => {
                    self.store.delete(&inventory)?;
                    return Ok(Expiry {
                        live: None,
                        removed: false,
                    });
                }
                Some(State::Live(pit, generation)) => {
                    if !pit.expired_at(now) {
                        return Ok(Expiry {
                            live: Some(pit),
                            removed: false,
                        });
                    }
                    if matches!(
                        self.store.swap_register(&state_key, generation, RELEASED)?,
                        SwapOutcome::Applied(_)
                    ) {
                        self.store.delete(&inventory)?;
                        return Ok(Expiry {
                            live: None,
                            removed: true,
                        });
                    }
                }
            }
        }
        Err(MetaError::Contended)
    }

    /// Visits every inventory id a page at a time.
    fn for_each_id(
        &self,
        mut each: impl FnMut(&str) -> Result<(), MetaError>,
    ) -> Result<(), MetaError> {
        // ALLOC: the listing prefix, once a pass.
        let prefix = [self.base.as_str(), POINTS_IN_TIME_PREFIX].concat();
        let mut after: Option<String> = None;
        loop {
            // ALLOC: one page of ids, at most MAX_LIST_KEYS.
            let mut ids: Vec<String> = Vec::new();
            // LIST: the inventory of points in time, one page at a time; an
            // explicit listing whose size is the views kept, which are
            // bounded by their deadlines.
            let page = self.store.list_prefix(
                &prefix,
                after.as_deref(),
                MAX_LIST_KEYS,
                &mut |key, _| {
                    if let Some(id) = key.strip_prefix(prefix.as_str()) {
                        // ALLOC: one id a key of the page, at most MAX_LIST_KEYS.
                        ids.push(id.to_owned());
                    }
                },
            )?;
            for id in &ids {
                if looks_like_an_id(id) {
                    each(id)?;
                }
            }
            match ids.pop() {
                Some(last) if page.truncated => {
                    after = Some(
                        // ALLOC: the key to resume after, once a page.
                        [prefix.as_str(), &last].concat(),
                    )
                }
                _ => return Ok(()),
            }
        }
    }

    /// Every view that has not expired, claiming the expiry of those that
    /// have. A view that cannot be read stands in as a placeholder naming no
    /// index and expiring never, which a collector reads as "delete
    /// nothing": what it holds is unknowable.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the listing fails.
    pub fn live(&self, buf: &mut [u8]) -> Result<Vec<PointInTime>, MetaError> {
        let now = (self.clock)();
        // ALLOC: one view a live id, each at most MAX_POINT_IN_TIME_BYTES; the
        // views a deployment keeps, ended by their deadlines. `buf` is the
        // caller's, and a record longer than it reads as a placeholder.
        let mut live = Vec::new();
        self.for_each_id(|id| {
            match self.expire(id, now, buf) {
                Ok(Expiry {
                    live: Some(pit), ..
                }) => live.push(pit),
                Ok(_) => {}
                Err(_) => live.push(PointInTime {
                    // ALLOC: the id of a view that could not be read.
                    id: id.to_owned(),
                    index: String::new(),
                    index_uuid: None,
                    expires_at_millis: i64::MAX.unsigned_abs(),
                    capturing: false,
                    // ALLOC: none, an empty vector holds no memory.
                    shards: Vec::new(),
                }),
            }
            Ok(())
        })?;
        Ok(live)
    }

    /// Removes the views that have expired; returns how many. One that
    /// cannot be read is left alone.
    ///
    /// # Errors
    ///
    /// [`MetaError::Blob`] when the listing fails.
    pub fn reap(&self, buf: &mut [u8]) -> Result<usize, MetaError> {
        let now = (self.clock)();
        let mut removed = 0usize;
        self.for_each_id(|id| {
            if let Ok(Expiry { removed: true, .. }) = self.expire(id, now, buf) {
                removed = removed.saturating_add(1);
            }
            Ok(())
        })?;
        Ok(removed)
    }
}
