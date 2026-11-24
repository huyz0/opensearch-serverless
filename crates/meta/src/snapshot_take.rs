//! Reserving a snapshot's name, publishing its protective marker, finishing
//! it and cleaning up after a failed take, as the shell's
//! `SnapshotStore.create`, `finish` and `remove` do.
//!
//! A snapshot is the name's state register (`snapshot-states/<hash>`, a
//! compare-and-swap value) and a discovery marker blob under `snapshots/`.
//! Fencing is the register's compare-and-swap and the marker's conditional
//! create; there is no lock. Every call builds into the one buffer it is
//! given, so a take holds a single record's worth of memory.

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, CreateOutcome, RegisterRead, SliceSource, SwapOutcome};
use oss_format::{
    FormatError, SnapshotDraft, SnapshotState, sha256_hex, snapshot_legacy_key, snapshot_state_key,
};

const SCRATCH: usize = 4 << 10;
/// How often a reservation or a release retries a lost swap.
const ATTEMPTS: usize = 16;
/// The longest `<repo>#<name>` the Java resolves as one file name.
const MAX_KEY_BYTES: usize = 255;

/// Why a step of a take failed.
#[derive(Debug)]
pub enum TakeError {
    /// A name made no key, or a record could not be written.
    Format(FormatError),
    /// The draft names no storage generation.
    NoGeneration,
    /// The store failed.
    Blob(BlobError),
    /// The name already holds a snapshot.
    AlreadyExists,
    /// `<repo>#<name>` is over 255 bytes: refused, never listed.
    KeyTooLong,
    /// The take was overtaken: another writer changed the name.
    Conflict(&'static str),
    /// A record or register value does not fit the buffer; the value is the
    /// length it needs, or the buffer's length when that is not known.
    RecordTooLarge(u64),
}

impl std::fmt::Display for TakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Format(e) => write!(f, "{e}"),
            Self::Blob(e) => write!(f, "{e}"),
            Self::NoGeneration => write!(f, "a take names a storage generation"),
            Self::AlreadyExists => write!(f, "the snapshot already exists"),
            Self::KeyTooLong => write!(f, "the repository and snapshot names are too long"),
            Self::Conflict(why) => write!(f, "{why}"),
            Self::RecordTooLarge(n) => write!(f, "a snapshot record of {n} bytes does not fit"),
        }
    }
}

impl std::error::Error for TakeError {}

impl From<FormatError> for TakeError {
    fn from(e: FormatError) -> Self {
        match e {
            FormatError::TooLarge { actual, .. } => Self::RecordTooLarge(actual),
            other => Self::Format(other),
        }
    }
}

impl From<BlobError> for TakeError {
    fn from(e: BlobError) -> Self {
        Self::Blob(e)
    }
}

/// A name this take holds: what finishing and cleaning up need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    state_key: String,
    marker: String,
    marker_key: String,
}

impl Reservation {
    /// The discovery marker's name under `snapshots/`.
    #[must_use]
    pub fn marker(&self) -> &str {
        &self.marker
    }
}

/// Takes the steps of one snapshot take against the store.
pub struct SnapshotTaker {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl SnapshotTaker {
    /// A taker over `store` under `base`.
    #[must_use]
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Self {
        Self {
            store,
            // ALLOC: one name, once a node.
            base: base.to_owned(),
        }
    }

    /// Reserves the name of `draft` (which names its indices and no
    /// commits, under a fresh storage generation) by creating its register,
    /// or swapping it over a released one, up to 16 tries. `value` is the
    /// buffer the register is read into and the value built in.
    ///
    /// Cost per try: a register GET, a HEAD of the legacy record when the
    /// register is absent, and one create or swap.
    ///
    /// # Errors
    ///
    /// [`TakeError::AlreadyExists`] when the name holds a snapshot;
    /// [`TakeError::KeyTooLong`]; [`TakeError::Conflict`] after 16 lost
    /// tries; [`TakeError`] otherwise.
    pub fn reserve(
        &self,
        draft: &SnapshotDraft,
        value: &mut [u8],
    ) -> Result<Reservation, TakeError> {
        let key = [draft.repo.as_str(), "#", draft.name.as_str()].concat();
        if key.len() > MAX_KEY_BYTES {
            return Err(TakeError::KeyTooLong);
        }
        let generation = draft
            .storage_generation
            .as_deref()
            .ok_or(TakeError::NoGeneration)?;
        let state_key = snapshot_state_key(&self.base, &draft.repo, &draft.name)?;
        // ALLOC: one marker, about 160 bytes, once a take.
        let marker = format!(
            "v2-{}#{}#{generation}",
            sha256_hex(&draft.repo),
            sha256_hex(&key)
        );
        // ALLOC: one key, once a take.
        let marker_key = [self.base.as_str(), "snapshots/", &marker].concat();
        let legacy = snapshot_legacy_key(&self.base, &draft.repo, &draft.name)?;
        for _ in 0..ATTEMPTS {
            let swap_over = match self.read(&state_key, value)? {
                Some(read) => match state(value, read)? {
                    SnapshotState::Held { .. } => return Err(TakeError::AlreadyExists),
                    SnapshotState::Released => Some(read.generation),
                },
                None => {
                    if self.legacy_exists(&legacy)? {
                        return Err(TakeError::AlreadyExists);
                    }
                    None
                }
            };
            let len = SnapshotState::write_held_draft(&marker, draft, false, value)?;
            let bytes = value.get(..len).unwrap_or_default();
            let outcome = match swap_over {
                Some(at) => self.store.swap_register(&state_key, at, bytes)?,
                None => self.store.create_register(&state_key, bytes)?,
            };
            if matches!(outcome, SwapOutcome::Applied(_)) {
                return Ok(Reservation {
                    state_key,
                    marker,
                    marker_key,
                });
            }
        }
        Err(TakeError::Conflict(
            "snapshot reservation conflicted repeatedly",
        ))
    }

    /// Writes the protective marker, `protective` being the record with no
    /// mapping, settings or shards, by a conditional create, then confirms
    /// the register still names this marker. Cost: one PUT and one GET.
    ///
    /// # Errors
    ///
    /// [`TakeError::Conflict`] when the marker exists or the snapshot was
    /// deleted meanwhile; [`TakeError`] otherwise.
    pub fn publish_marker(
        &self,
        reservation: &Reservation,
        protective: &SnapshotDraft,
        value: &mut [u8],
    ) -> Result<(), TakeError> {
        let len = protective.write(value)?;
        let mut scratch = [0u8; SCRATCH];
        let body = value.get(..len).unwrap_or_default();
        match self.store.create_if_absent(
            &reservation.marker_key,
            &mut SliceSource::new(body),
            &mut scratch,
        )? {
            CreateOutcome::Created => {}
            CreateOutcome::AlreadyExists => {
                return Err(TakeError::Conflict("the snapshot marker already exists"));
            }
        }
        match self.holding(reservation, value)? {
            Some((_, Some(false))) => Ok(()),
            _ => Err(TakeError::Conflict(
                "snapshot was deleted while publishing its capture marker",
            )),
        }
    }

    /// Finishes the snapshot: the register's value becomes `finished` with
    /// the finished flag, by a swap from the generation just read. Cost: one
    /// GET and one swap.
    ///
    /// # Errors
    ///
    /// [`TakeError::Conflict`] when the capture is no longer active (the
    /// register does not hold this unfinished take) or changed meanwhile;
    /// [`TakeError::RecordTooLarge`] when the record does not fit `value`.
    pub fn finish(
        &self,
        reservation: &Reservation,
        finished: &SnapshotDraft,
        value: &mut [u8],
    ) -> Result<(), TakeError> {
        let Some((at, Some(false))) = self.holding(reservation, value)? else {
            return Err(TakeError::Conflict("snapshot capture is no longer active"));
        };
        let len = SnapshotState::write_held_draft(&reservation.marker, finished, true, value)?;
        let bytes = value.get(..len).unwrap_or_default();
        match self
            .store
            .swap_register(&reservation.state_key, at, bytes)?
        {
            SwapOutcome::Applied(_) => Ok(()),
            SwapOutcome::Conflict(_) => Err(TakeError::Conflict(
                "snapshot changed while finishing capture",
            )),
        }
    }

    /// Releases this take's name and removes its marker, after a failure:
    /// the register is released only while it still names this take's
    /// marker, and the marker is deleted once the register no longer names
    /// it, so a delete and a re-take of the name during a failing take is
    /// never wiped. Release first: a marker without a register is skipped.
    ///
    /// # Errors
    ///
    /// [`TakeError`]; the caller keeps the failure that led here.
    pub fn abort(&self, reservation: &Reservation, value: &mut [u8]) -> Result<(), TakeError> {
        for _ in 0..ATTEMPTS {
            let held = self.holding(reservation, value)?;
            // A register that names this marker finished (a lost reply to
            // the finishing swap): the snapshot stands, and so does its marker.
            if matches!(held, Some((_, Some(true)))) {
                return Ok(());
            }
            if let Some((at, Some(false))) = held {
                let released = self.store.swap_register(
                    &reservation.state_key,
                    at,
                    SnapshotState::released_value(),
                )?;
                if !matches!(released, SwapOutcome::Applied(_)) {
                    continue;
                }
            }
            self.store.delete(&reservation.marker_key)?;
            return Ok(());
        }
        Err(TakeError::Conflict(
            "snapshot release conflicted repeatedly",
        ))
    }

    /// Reads the register: its generation and, when it names this take's
    /// marker, whether it is finished; `None` when there is no register.
    fn holding(
        &self,
        reservation: &Reservation,
        value: &mut [u8],
    ) -> Result<Option<(u64, Option<bool>)>, TakeError> {
        let Some(read) = self.read(&reservation.state_key, value)? else {
            return Ok(None);
        };
        let ours = match state(value, read)? {
            SnapshotState::Held {
                marker, finished, ..
            } if marker == reservation.marker => Some(finished),
            _ => None,
        };
        Ok(Some((read.generation, ours)))
    }

    fn read(&self, state_key: &str, value: &mut [u8]) -> Result<Option<RegisterRead>, TakeError> {
        let cap = value.len();
        match self.store.read_register(state_key, value) {
            Ok(read) => Ok(read),
            Err(BlobError::InvalidArgument(_)) => Err(TakeError::RecordTooLarge(
                u64::try_from(cap).unwrap_or(u64::MAX),
            )),
            Err(e) => Err(e.into()),
        }
    }

    fn legacy_exists(&self, legacy: &str) -> Result<bool, TakeError> {
        match self.store.head(legacy) {
            Ok(found) => Ok(found.is_some()),
            // A key this store cannot name holds no legacy snapshot.
            Err(BlobError::InvalidArgument(_)) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn state(value: &[u8], read: RegisterRead) -> Result<SnapshotState<'_>, TakeError> {
    Ok(SnapshotState::parse(
        value.get(..read.len).unwrap_or_default(),
    )?)
}
