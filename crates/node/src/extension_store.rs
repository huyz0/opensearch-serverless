//! Extension modules in the store: content-addressed, written once, read
//! once a node ([ADR 0052](../../../docs/adr/0052-extension-modules-on-a-wasm-abi.md)).

use std::sync::Arc;

use oss_blob::{BlobError, BlobStore, ByteRange, CreateOutcome, SliceSource};
use oss_ext::MAX_MODULE_BYTES;
use oss_ext_wasm::{Host, LoadError, Module, Reservation, Reserver};
use oss_mem::{MemoryBudget, Pool};
use sha2::{Digest, Sha256};

/// The buffer a module is written and read through.
const IO_BUFFER: usize = 64 * 1024;

/// The key a module with this hash lives under.
pub fn key(base: &str, hash: &str) -> String {
    format!("{base}ext/modules/{hash}.wasm")
}

/// The lowercase hex SHA-256 of `bytes`.
pub fn hash_of(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    // ALLOC: one 64-character hash an upload or a load.
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A hash as a module key names it: 64 lowercase hex digits.
pub fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Charges the host's reservations to the `extensions` pool.
pub struct PoolReserver(pub MemoryBudget);

impl Reserver for PoolReserver {
    fn reserve(&self, bytes: u64) -> Option<Reservation> {
        let bytes = usize::try_from(bytes).ok()?;
        let held = self.0.try_reserve(Pool::Extensions, bytes).ok()?;
        // ALLOC: one boxed reservation a call.
        Some(Box::new(held))
    }
}

/// Why a module was not stored or loaded.
#[derive(Debug)]
pub enum StoreError {
    /// The store failed.
    Blob(BlobError),
    /// The module is refused.
    Load(LoadError),
    /// The bytes under a key do not hash to it.
    Mismatch,
}

/// Writes `bytes` under their hash if no module is there: one PUT the first
/// time and none for a repeat. Whether this call wrote it.
pub fn write_once(
    store: &dyn BlobStore,
    base: &str,
    hash: &str,
    bytes: &[u8],
) -> Result<bool, StoreError> {
    let k = key(base, hash);
    // A repeat is told by a HEAD, so it adds no PUT.
    if store.head(&k).map_err(StoreError::Blob)?.is_some() {
        return Ok(false);
    }
    let mut buf = vec![0_u8; IO_BUFFER]; // ALLOC: IO_BUFFER, once an upload.
    match store
        .create_if_absent(&k, &mut SliceSource::new(bytes), &mut buf)
        .map_err(StoreError::Blob)?
    {
        CreateOutcome::Created => Ok(true),
        CreateOutcome::AlreadyExists => Ok(false),
    }
}

/// Reads the module `hash` with one GET into a buffer reserved from the
/// `extensions` pool, checks it hashes to its key, and compiles it.
pub fn load(
    store: &dyn BlobStore,
    base: &str,
    hash: &str,
    host: &Host,
    budget: &MemoryBudget,
) -> Result<Module, StoreError> {
    let _held = budget
        .try_reserve(Pool::Extensions, MAX_MODULE_BYTES as usize)
        .map_err(|_| StoreError::Load(LoadError::Busy))?;
    // ALLOC: at most MAX_MODULE_BYTES, reserved above.
    let mut bytes: Vec<u8> = Vec::new();
    let mut scratch = vec![0_u8; IO_BUFFER]; // ALLOC: IO_BUFFER, once a load.
    let mut sink = |chunk: &[u8]| {
        if bytes.len().saturating_add(chunk.len()) > MAX_MODULE_BYTES as usize {
            return Err(BlobError::InvalidRange);
        }
        bytes.extend_from_slice(chunk);
        Ok(())
    };
    store
        .get(&key(base, hash), ByteRange::Full, &mut scratch, &mut sink)
        .map_err(StoreError::Blob)?;
    if hash_of(&bytes) != hash {
        return Err(StoreError::Mismatch);
    }
    host.load(&bytes, Arc::new(PoolReserver(budget.clone())))
        .map_err(StoreError::Load)
}
