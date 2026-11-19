//! Points in time: the register that says whether a frozen view lives, and
//! the inventory entry that makes it findable.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/PointInTimeStore.java`.
//! Each view has an authoritative state register, `pit-states/<id>`, holding
//! its [`PointInTime`] or the [`RELEASED`] tombstone, and an inventory marker,
//! `pits/<id>`, that names the index it pins and a deadline no collector
//! reads as past, so older collectors that read only the inventory keep the
//! index's files. Released ids keep their tombstone: a delayed request can
//! neither bring an id back nor release a later view of the same id. Every
//! change is a compare-and-swap of the state register, never a lock.
//!
//! A view written by a node that predates the state register has only its
//! inventory blob; reading it adopts it into a register first.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange, CreateOutcome, SliceSource, SwapOutcome};
use oss_format::{FormatError, PointInTime, RELEASED, point_in_time_key, point_in_time_state_key};

use crate::error::MetaError;

mod capture;
mod sweep;

/// How many times a change reads the state and swaps before giving up to
/// contention, as the Java's.
const ATTEMPTS: u32 = 16;

pub use capture::{CaptureError, new_id};

/// The clock a view's deadline is judged by, in milliseconds.
pub type PitClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The state of one id.
enum State {
    /// The id was released: nothing may bring it back.
    Released,
    /// The view, and the generation of the register holding it.
    Live(PointInTime, u64),
}

/// What expiring one id found.
struct Expiry {
    live: Option<PointInTime>,
    removed: bool,
}

/// The points in time of one deployment.
pub struct PointInTimeStore {
    store: Arc<dyn BlobStore>,
    base: String,
    clock: PitClock,
}

impl PointInTimeStore {
    /// The store under `base` (empty, or ending in `/`) of `store`, judging
    /// deadlines by `clock`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end in
    /// `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str, clock: PitClock) -> Result<Self, MetaError> {
        // The base is checked once, by deriving a key from it.
        point_in_time_key(base, "AAAAAAAAAAAAAAAAAAAAAA")?;
        Ok(Self {
            store,
            // ALLOC: the base, once a store.
            base: base.to_owned(),
            clock,
        })
    }

    fn inventory_key(&self, id: &str) -> Result<String, MetaError> {
        Ok(point_in_time_key(&self.base, id)?)
    }

    fn state_key(&self, id: &str) -> Result<String, MetaError> {
        Ok(point_in_time_state_key(&self.base, id)?)
    }

    /// Records a new view or capture marker (`buf` holds one record, which
    /// [`oss_format::MAX_POINT_IN_TIME_BYTES`] bounds; a longer one is never
    /// readable back): it must not exist, even as a
    /// legacy inventory entry. Three requests: a HEAD, the register's
    /// create and the marker's.
    ///
    /// # Errors
    ///
    /// [`MetaError::AlreadyExists`] when the id is taken;
    /// [`MetaError::Format`] for an id that is not 22 URL-safe characters or
    /// a record that does not fit `buf`; [`MetaError::Blob`] when the store
    /// fails. A failure after the register was created releases it again.
    pub fn create(&self, pit: &PointInTime, buf: &mut [u8]) -> Result<(), MetaError> {
        let inventory = self.inventory_key(&pit.id)?;
        let state = self.state_key(&pit.id)?;
        if self.store.head(&inventory)?.is_some() {
            return Err(MetaError::AlreadyExists);
        }
        let len = encode(pit, buf)?;
        let generation = match self
            .store
            .create_register(&state, buf.get(..len).unwrap_or_default())?
        {
            SwapOutcome::Applied(generation) => generation,
            SwapOutcome::Conflict(_) => return Err(MetaError::AlreadyExists),
        };
        match self.write_marker(pit, true, buf) {
            Ok(()) => Ok(()),
            Err(failure) => {
                // Best effort: the failure being reported is the first. A
                // marker that is already there belongs to another creator
                // of the id, and is not ours to remove.
                let ours = !matches!(failure, MetaError::AlreadyExists);
                let _ = self.abandon(&inventory, &state, generation, ours, buf);
                Err(failure)
            }
        }
    }

    /// Releases a view whose marker could not be written, and removes the
    /// marker only when this release won or the id was already released.
    fn abandon(
        &self,
        inventory: &str,
        state: &str,
        generation: u64,
        remove_marker: bool,
        buf: &mut [u8],
    ) -> Result<(), MetaError> {
        let may_delete = match self.store.swap_register(state, generation, RELEASED)? {
            SwapOutcome::Applied(_) => true,
            SwapOutcome::Conflict(_) => {
                !matches!(self.read_state(state, buf)?, Some(State::Live(..)))
            }
        };
        if may_delete && remove_marker {
            self.store.delete(inventory)?;
        }
        Ok(())
    }

    /// Writes the inventory marker of `pit`: only if there is none when
    /// `create`, over whatever is there otherwise.
    fn write_marker(
        &self,
        pit: &PointInTime,
        create: bool,
        buf: &mut [u8],
    ) -> Result<(), MetaError> {
        let key = self.inventory_key(&pit.id)?;
        let len = pit.marker().write(buf).map_err(MetaError::Format)?;
        let body = buf.get(..len).unwrap_or_default();
        let mut scratch = [0u8; 4096];
        if create {
            match self
                .store
                .create_if_absent(&key, &mut SliceSource::new(body), &mut scratch)?
            {
                CreateOutcome::Created => Ok(()),
                _ => Err(MetaError::AlreadyExists),
            }
        } else {
            Ok(self
                .store
                .put(&key, &mut SliceSource::new(body), &mut scratch)?)
        }
    }

    /// Reads a state register: `None` when there is none.
    fn read_state(&self, key: &str, buf: &mut [u8]) -> Result<Option<State>, MetaError> {
        let Some(read) = self.store.read_register(key, buf)? else {
            return Ok(None);
        };
        let value = buf.get(..read.len).unwrap_or_default();
        if value == RELEASED {
            return Ok(Some(State::Released));
        }
        Ok(Some(State::Live(
            PointInTime::parse(value)?,
            read.generation,
        )))
    }

    /// The state of `id`, adopting a legacy inventory entry into a register
    /// when there is no register (state presence is monotonic).
    fn state(&self, id: &str, buf: &mut [u8]) -> Result<Option<State>, MetaError> {
        let key = self.state_key(id)?;
        let found = self.read_state(&key, buf)?;
        if let Some(state) = found {
            return Ok(Some(self.checked(id, state)?));
        }
        let Some(len) = self.read_inventory(id, buf)? else {
            return Ok(None);
        };
        let legacy = PointInTime::parse(buf.get(..len).unwrap_or_default())?;
        if legacy.id != id {
            return Err(identity());
        }
        // Whoever creates first wins; both wrote the same bytes.
        self.store
            .create_register(&key, buf.get(..len).unwrap_or_default())?;
        match self.read_state(&key, buf)? {
            Some(state) => Ok(Some(self.checked(id, state)?)),
            None => Err(MetaError::InvalidArgument(
                "point in time state disappeared",
            )),
        }
    }

    fn checked(&self, id: &str, state: State) -> Result<State, MetaError> {
        match state {
            State::Live(pit, _) if pit.id != id => Err(identity()),
            other => Ok(other),
        }
    }

    /// Reads an inventory blob into the front of `buf`: its length, or
    /// `None` when there is none.
    fn read_inventory(&self, id: &str, buf: &mut [u8]) -> Result<Option<usize>, MetaError> {
        let key = self.inventory_key(id)?;
        let mut scratch = [0u8; 4096];
        let mut filled = 0usize;
        let mut sink = |chunk: &[u8]| -> Result<(), BlobError> {
            let end = filled.saturating_add(chunk.len());
            let to = buf.get_mut(filled..end).ok_or(BlobError::InvalidArgument(
                "a point in time does not fit the buffer",
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

    /// Replaces a capture marker with the finished view under the same id:
    /// the id already pins the index, and a second one would leave the marker
    /// to expire on its own beside the real view.
    ///
    /// # Errors
    ///
    /// [`MetaError::InvalidArgument`] when the capture is no longer active,
    /// `pit` is itself a capture, its index or uuid is not the capture's, or
    /// the register changed while finishing; [`MetaError::Format`] and
    /// [`MetaError::Blob`] as [`PointInTimeStore::create`].
    pub fn finish(&self, pit: &PointInTime, buf: &mut [u8]) -> Result<(), MetaError> {
        let current = self.state(&pit.id, buf)?;
        let Some(State::Live(stored, generation)) = current else {
            return Err(inactive());
        };
        if !stored.capturing || pit.capturing {
            return Err(inactive());
        }
        if stored.index != pit.index || stored.index_uuid != pit.index_uuid {
            return Err(MetaError::InvalidArgument(
                "point in time capture identity changed",
            ));
        }
        let view = pit.clone().with_expiry(stored.expires_at_millis);
        let len = encode(&view, buf)?;
        let state = self.state_key(&pit.id)?;
        match self
            .store
            .swap_register(&state, generation, buf.get(..len).unwrap_or_default())?
        {
            SwapOutcome::Applied(_) => Ok(()),
            SwapOutcome::Conflict(_) => Err(MetaError::InvalidArgument(
                "point in time capture changed while finishing",
            )),
        }
    }

    /// Moves a view's deadline later: to `expires_at_millis` when that is
    /// past the stored one. `expected_expiry`, when given, makes it
    /// conditional on the deadline the caller saw, so one racing extension
    /// wins and the other is told. Returns whether the view is live and now
    /// holds at least that deadline (or, conditional, whether this call won).
    ///
    /// # Errors
    ///
    /// [`MetaError::Contended`] when 16 attempts all lost an unconditional
    /// extension; [`MetaError::Format`] and [`MetaError::Blob`] as
    /// [`PointInTimeStore::create`].
    pub fn extend(
        &self,
        id: &str,
        expires_at_millis: u64,
        expected_expiry: Option<u64>,
        buf: &mut [u8],
    ) -> Result<bool, MetaError> {
        let state_key = self.state_key(id)?;
        for _ in 0..ATTEMPTS {
            let Some(State::Live(stored, generation)) = self.state(id, buf)? else {
                return Ok(false);
            };
            if stored.capturing
                || stored.expired_at((self.clock)())
                || expected_expiry.is_some_and(|seen| stored.expires_at_millis != seen)
            {
                return Ok(false);
            }
            let deadline = stored.expires_at_millis.max(expires_at_millis);
            if deadline == stored.expires_at_millis {
                return Ok(true);
            }
            self.protect_legacy_marker(&stored, buf)?;
            let len = stored
                .clone()
                .with_expiry(deadline)
                .write(buf)
                .map_err(MetaError::Format)?;
            let value = buf.get(..len).unwrap_or_default();
            if matches!(
                self.store.swap_register(&state_key, generation, value)?,
                SwapOutcome::Applied(_)
            ) {
                return Ok(true);
            }
            if expected_expiry.is_some() {
                return Ok(false);
            }
        }
        Err(MetaError::Contended)
    }

    /// Makes sure the inventory marker pins the index until the deadline a
    /// collector that reads only the inventory cannot see move.
    fn protect_legacy_marker(&self, pit: &PointInTime, buf: &mut [u8]) -> Result<(), MetaError> {
        if let Some(len) = self.read_inventory(&pit.id, buf)? {
            let marker = PointInTime::parse(buf.get(..len).unwrap_or_default())?;
            if marker.capturing && marker.expires_at_millis == i64::MAX.unsigned_abs() {
                return Ok(());
            }
        }
        // Missing: a concurrent release may have removed it; the state
        // register's swap still stops a resurrection.
        self.write_marker(pit, false, buf)
    }

    /// The view, or `None` when it never existed, was released, or is only a
    /// capture marker so far. One request, or three for a legacy entry.
    ///
    /// # Errors
    ///
    /// As [`PointInTimeStore::create`], and [`MetaError::Format`] for a
    /// record that is malformed.
    pub fn read(&self, id: &str, buf: &mut [u8]) -> Result<Option<PointInTime>, MetaError> {
        Ok(match self.state(id, buf)? {
            Some(State::Live(pit, _)) if !pit.capturing => Some(pit),
            _ => None,
        })
    }

    /// Releases a view so the collector stops holding its files: the state
    /// register becomes the tombstone, then the marker goes. Returns whether
    /// there was a live one.
    ///
    /// # Errors
    ///
    /// [`MetaError::Contended`] when 16 attempts all lost; otherwise as
    /// [`PointInTimeStore::create`].
    pub fn release(&self, id: &str, buf: &mut [u8]) -> Result<bool, MetaError> {
        let state_key = self.state_key(id)?;
        let inventory = self.inventory_key(id)?;
        for _ in 0..ATTEMPTS {
            let Some(State::Live(_, generation)) = self.state(id, buf)? else {
                return Ok(false);
            };
            if matches!(
                self.store.swap_register(&state_key, generation, RELEASED)?,
                SwapOutcome::Applied(_)
            ) {
                self.store.delete(&inventory)?;
                return Ok(true);
            }
        }
        Err(MetaError::Contended)
    }
}

/// Writes `pit` into `buf` and checks that a reader takes what was written:
/// a record the store would create unreadable could never be released or
/// expired, and would stop a collector deleting anything.
fn encode(pit: &PointInTime, buf: &mut [u8]) -> Result<usize, MetaError> {
    let len = pit.write(buf).map_err(MetaError::Format)?;
    PointInTime::parse(buf.get(..len).unwrap_or_default())?;
    Ok(len)
}

fn identity() -> MetaError {
    MetaError::Format(FormatError::Corrupt {
        what: "point in time",
        reason: "its identity does not match its key",
    })
}

fn inactive() -> MetaError {
    MetaError::InvalidArgument("point in time capture is no longer active")
}
