//! A tenant's alerting monitors: bodies as immutable content blobs and one
//! index register that names them, with each monitor's enabled flag and
//! schedule, so a change is one compare-and-swap and a scheduler reads every
//! monitor's schedule in one request.
//!
//! A put writes the body under a new key, then swaps the index. After a put
//! of an enabled monitor [`Monitors::put`] registers the tenant in the
//! registry (always swapping its shard); after a delete or disable that
//! leaves no enabled monitor [`Monitors::delete`] and [`Monitors::put`]
//! remove it, asking the index again after the shard is read (see
//! [`AlertRegistry`]). Nothing lists.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobError, BlobStore, ByteRange, SliceSource, SwapOutcome};
use oss_format::{
    MAX_MONITOR_BYTES, MAX_MONITOR_INDEX_BYTES, MonitorEntry, MonitorIndex, check_tenant_id,
    monitor_content_key, monitor_index_key,
};

use crate::error::MetaError;
use crate::pit::new_id;
use crate::schedule_registry::AlertRegistry;

/// How many times a change reads the index and swaps before giving up.
const ATTEMPTS: u32 = 10;

/// What a [`Monitors::put`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorPut {
    /// The index's change count after the put.
    pub version: i64,
    /// Whether the id was already there.
    pub replaced: bool,
}

/// One tenant's monitors.
pub struct Monitors {
    store: Arc<dyn BlobStore>,
    base: String,
    tenant: String,
    index_key: String,
    registry: AlertRegistry,
}

impl Monitors {
    /// The monitors of `tenant` under `base` of `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for an invalid tenant id or base.
    pub fn new(store: &Arc<dyn BlobStore>, base: &str, tenant: &str) -> Result<Self, MetaError> {
        check_tenant_id(tenant)?;
        Ok(Self {
            store: Arc::clone(store),
            // ALLOC: one copy of the base and tenant, once.
            base: base.to_owned(),
            tenant: tenant.to_owned(),
            index_key: monitor_index_key(base, tenant)?,
            registry: AlertRegistry::new(Arc::clone(store), base)?,
        })
    }

    /// The tenant's index and the generation it was read at: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn index(&self) -> Result<MonitorIndex, MetaError> {
        Ok(self.read()?.0)
    }

    fn read(&self) -> Result<(MonitorIndex, u64), MetaError> {
        // ALLOC: one index's worth, the most a register here holds, per read.
        let mut buf = vec![0u8; MAX_MONITOR_INDEX_BYTES];
        match self.store.read_register(&self.index_key, &mut buf)? {
            None => Ok((MonitorIndex::new(0), ABSENT_GENERATION)),
            Some(read) => Ok((
                MonitorIndex::parse(buf.get(..read.len).unwrap_or_default())?,
                read.generation,
            )),
        }
    }

    fn swap(&self, index: &MonitorIndex, generation: u64) -> Result<bool, MetaError> {
        // ALLOC: one index's worth, the most a register here holds, per swap.
        let mut buf = vec![0u8; MAX_MONITOR_INDEX_BYTES];
        let len = index.write(&mut buf)?;
        let value = buf.get(..len).unwrap_or_default();
        let outcome = if generation == ABSENT_GENERATION {
            self.store.create_register(&self.index_key, value)?
        } else {
            self.store
                .swap_register(&self.index_key, generation, value)?
        };
        Ok(matches!(outcome, SwapOutcome::Applied(_)))
    }

    /// Reads a monitor's body into the front of `body_buf` and returns its
    /// entry and the body's length, or `None` for an id the tenant lacks: the
    /// index and then the body, two requests. A body the index names that is
    /// gone was replaced since; the index is read once more.
    ///
    /// # Errors
    ///
    /// [`MetaError::MissingContent`] when it is still gone;
    /// [`MetaError::Format`]; [`MetaError::Blob`] (a body that does not fit).
    pub fn get(
        &self,
        id: &str,
        body_buf: &mut [u8],
    ) -> Result<Option<(MonitorEntry, usize)>, MetaError> {
        for _ in 0..2 {
            let (index, _) = self.read()?;
            let Some(entry) = index.get(id) else {
                return Ok(None);
            };
            if let Some(len) = self.read_body(&entry.content, body_buf)? {
                return Ok(Some((entry.clone(), len)));
            }
        }
        // ALLOC: one id, once, on a damaged set.
        Err(MetaError::MissingContent(id.to_owned()))
    }

    fn read_body(&self, content: &str, body_buf: &mut [u8]) -> Result<Option<usize>, MetaError> {
        let key = monitor_content_key(&self.base, &self.tenant, content)?;
        let mut scratch = [0u8; 4096];
        let mut filled = 0usize;
        let mut sink = |chunk: &[u8]| -> Result<(), BlobError> {
            let end = filled.saturating_add(chunk.len());
            let to = body_buf
                .get_mut(filled..end)
                .ok_or(BlobError::InvalidArgument(
                    "a monitor does not fit the buffer",
                ))?;
            to.copy_from_slice(chunk);
            filled = end;
            Ok(())
        };
        match self
            .store
            .get(&key, ByteRange::Full, &mut scratch, &mut sink)
        {
            Ok(_) => Ok(Some(filled)),
            Err(BlobError::NotFound) => Ok(None),
            Err(other) => Err(other.into()),
        }
    }

    /// Stores `body` as monitor `id` with its schedule, replacing one of that
    /// id unless `require_absent`, and registers the tenant when the monitor
    /// is enabled. The body first under a new key, one swap of the index
    /// naming it, then the registry's swap; the replaced body goes last. A put that
    /// leaves the tenant with no enabled monitor runs the registry's removal.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidArgument`] for a body over [`MAX_MONITOR_BYTES`];
    /// [`MetaError::AlreadyExists`] under `require_absent`;
    /// [`MetaError::Format`] for an invalid id or schedule, a full index or a
    /// full registry shard; [`MetaError::Contended`]; [`MetaError::Blob`].
    pub fn put(
        &self,
        id: &str,
        body: &[u8],
        enabled: bool,
        schedule: &str,
        now_ms: i64,
        require_absent: bool,
    ) -> Result<MonitorPut, MetaError> {
        if body.len() > MAX_MONITOR_BYTES {
            return Err(MetaError::InvalidArgument(
                "a monitor is over its size bound",
            ));
        }
        let content = new_id()?;
        self.write_body(&content, body)?;
        let outcome = self.change_index(id, require_absent, |index| {
            index
                .insert(
                    id,
                    MonitorEntry {
                        // ALLOC: one blob key, once a put.
                        content: content.clone(),
                        enabled,
                        // ALLOC: one schedule, at most MAX_SCHEDULE_BYTES, once a put.
                        schedule: schedule.to_owned(),
                        updated_ms: now_ms,
                    },
                )
                .map_err(MetaError::from)
        });
        let (version, replaced, any_enabled) = match outcome {
            Ok(done) => done,
            Err(e) => {
                self.drop_body(&content);
                return Err(e);
            }
        };
        if let Some(old) = &replaced
            && old.content != content
        {
            // A put always writes a body under a new key, so no entry reads
            // the replaced one.
            self.drop_body(&old.content);
        }
        self.settle_registry(enabled || any_enabled, enabled)?;
        Ok(MonitorPut {
            version,
            replaced: replaced.is_some(),
        })
    }

    /// Removes monitor `id`: one swap of the index, then its body, then the
    /// registry's removal when no enabled monitor is left. Returns whether
    /// there was one.
    ///
    /// # Errors
    ///
    /// [`MetaError::Contended`]; [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn delete(&self, id: &str) -> Result<bool, MetaError> {
        for _ in 0..ATTEMPTS {
            let (mut index, generation) = self.read()?;
            let Some(old) = index.remove(id) else {
                return Ok(false);
            };
            index.version = index.version.saturating_add(1);
            if self.swap(&index, generation)? {
                if !index.shares(&old.content) {
                    self.drop_body(&old.content);
                }
                self.settle_registry(index.any_enabled(), false)?;
                return Ok(true);
            }
        }
        Err(MetaError::Contended)
    }

    /// Runs the registry's half of a change: a tenant with an enabled monitor
    /// is registered (`register` says this change enabled one, so the shard
    /// swap is made); one without is removed unless the index, asked after
    /// the shard is read, shows one.
    fn settle_registry(&self, has_enabled: bool, register: bool) -> Result<(), MetaError> {
        if register {
            return self.registry.register(&self.tenant);
        }
        if has_enabled {
            return Ok(());
        }
        self.registry
            .unregister_unless(&self.tenant, &mut || Ok(self.index()?.any_enabled()))
            .map(|_| ())
    }

    fn change_index(
        &self,
        id: &str,
        require_absent: bool,
        mut edit: impl FnMut(&mut MonitorIndex) -> Result<Option<MonitorEntry>, MetaError>,
    ) -> Result<(i64, Option<MonitorEntry>, bool), MetaError> {
        for _ in 0..ATTEMPTS {
            let (mut index, generation) = self.read()?;
            if require_absent && index.get(id).is_some() {
                return Err(MetaError::AlreadyExists);
            }
            let replaced = edit(&mut index)?;
            index.version = index.version.saturating_add(1);
            if self.swap(&index, generation)? {
                return Ok((index.version, replaced, index.any_enabled()));
            }
        }
        Err(MetaError::Contended)
    }

    fn write_body(&self, content: &str, body: &[u8]) -> Result<(), MetaError> {
        let key = monitor_content_key(&self.base, &self.tenant, content)?;
        let mut scratch = [0u8; 4096];
        self.store
            .put(&key, &mut SliceSource::new(body), &mut scratch)?;
        Ok(())
    }

    /// Deletes bytes no index names. A failure costs storage and nothing
    /// else: an unnamed blob is never read.
    fn drop_body(&self, content: &str) {
        if let Ok(key) = monitor_content_key(&self.base, &self.tenant, content) {
            let _ = self.store.delete(&key);
        }
    }
}
