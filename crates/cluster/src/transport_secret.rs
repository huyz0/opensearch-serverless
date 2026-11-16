//! The transport secret register, read by every node and cached for one
//! lease TTL: the keys the transport's request MAC is signed with.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/metadata/TransportSecretStore.java`
//! (`transportSecrets`) and `node/TransportSecretCache.java`. A read is one
//! GET; a deployment's first node mints the register with one conditional
//! create, so a fleet pays one PUT for its lifetime. A cache re-reads once a
//! lease TTL, so the requests scale with nodes, not with transport calls.
//! Rotation stays with the Java's operator action.

use std::sync::{Arc, Mutex, PoisonError};

use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{MAX_TRANSPORT_SECRET_BYTES, TransportSecret, transport_secret_key};
use oss_transport::auth::{SecretSource, Secrets};

use crate::error::ClusterError;
use crate::lease::LeaseClock;

/// The transport secret register of one deployment.
pub struct TransportSecretStore {
    store: Arc<dyn BlobStore>,
    key: String,
    clock: Arc<dyn LeaseClock>,
}

impl TransportSecretStore {
    /// The register under `base` (empty, or ending in `/`) of `store`.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Format`] for a base that is not empty and does not
    /// end in `/`.
    pub fn new(
        store: Arc<dyn BlobStore>,
        base: &str,
        clock: Arc<dyn LeaseClock>,
    ) -> Result<Self, ClusterError> {
        Ok(Self {
            store,
            key: transport_secret_key(base)?,
            clock,
        })
    }

    /// The secrets, at the register's generation: one read, or, when there
    /// is no register yet, a fresh 32-byte key created at generation 1 (one
    /// more request), and a read again when another node's create won.
    ///
    /// # Errors
    ///
    /// [`ClusterError::Blob`] when the store fails;
    /// [`ClusterError::Format`] for a malformed value;
    /// [`ClusterError::TransportSecret`] when the system has no randomness,
    /// or the register vanished between a lost create and the read after it.
    pub fn read(&self) -> Result<Secrets, ClusterError> {
        let mut buf = [0u8; MAX_TRANSPORT_SECRET_BYTES];
        if let Some(read) = self.read_once(&mut buf)? {
            return Ok(read);
        }
        let minted = TransportSecret {
            generation: 1,
            current: fresh_key()?,
            previous: None,
            rotated_at_millis: self.clock.now_millis(),
        };
        let len = minted.write(&mut buf)?;
        match self
            .store
            .create_register(&self.key, buf.get(..len).unwrap_or_default())?
        {
            SwapOutcome::Applied(generation) => Ok(secrets(minted, generation)),
            SwapOutcome::Conflict(_) => {
                self.read_once(&mut buf)?
                    .ok_or(ClusterError::TransportSecret(
                        "the transport secret vanished after a lost create",
                    ))
            }
        }
    }

    fn read_once(&self, buf: &mut [u8]) -> Result<Option<Secrets>, ClusterError> {
        let Some(read) = self.store.read_register(&self.key, buf)? else {
            return Ok(None);
        };
        let value = TransportSecret::parse(buf.get(..read.len).unwrap_or_default())?;
        Ok(Some(secrets(value, read.generation)))
    }
}

/// How soon a failed read is tried again, at most.
const RETRY_MILLIS: i64 = 1_000;

/// [`TransportSecretStore`] behind a cache, as the Java's
/// `TransportSecretCache`: read on first use, then again once a lease TTL
/// has passed, when the clock went back, or when the authenticator asks
/// for a generation it has not seen. One caller reads while the others go
/// on with the secrets held, so a slow store delays no transport call that
/// already has them.
///
/// Where the Java fails the call when a re-read fails, this keeps the
/// secrets it holds and tries again a second later, at most once a second
/// however many calls arrive: the keys change only by rotation, and the
/// previous key stays accepted for one TTL after it.
pub struct CachedTransportSecrets {
    store: TransportSecretStore,
    ttl_millis: i64,
    state: Mutex<Cached>,
    // Held by the one caller reading the register.
    reading: Mutex<()>,
}

struct Cached {
    secrets: Option<Arc<Secrets>>,
    // When the last read was tried, to see the clock go back.
    tried_at: i64,
    // When the next read is due.
    due_at: i64,
}

impl CachedTransportSecrets {
    /// A cache over `store`, re-read once every `ttl_millis`.
    pub fn new(store: TransportSecretStore, ttl_millis: i64) -> Self {
        Self {
            store,
            ttl_millis,
            state: Mutex::new(Cached {
                secrets: None,
                tried_at: i64::MIN,
                due_at: i64::MIN,
            }),
            reading: Mutex::new(()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Cached> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The secrets held when no read is due (or `force` asks for one), or
    /// `Err` with what is held.
    fn fresh(&self, force: bool) -> Result<Option<Arc<Secrets>>, Option<Arc<Secrets>>> {
        let state = self.state();
        let now = self.store.clock.now_millis();
        if !force && now >= state.tried_at && now <= state.due_at {
            Ok(state.secrets.clone())
        } else {
            Err(state.secrets.clone())
        }
    }

    fn get(&self, force: bool) -> Option<Arc<Secrets>> {
        let held = match self.fresh(force) {
            Ok(fresh) => return fresh,
            Err(held) => held,
        };
        let _reading = match self.reading.try_lock() {
            Ok(guard) => guard,
            // Another caller is reading: go on with what is held.
            Err(_) if held.is_some() => return held,
            // Nothing held yet: wait for the first read, and take it.
            Err(_) => {
                let guard = self.reading.lock().unwrap_or_else(PoisonError::into_inner);
                if let Ok(fresh) = self.fresh(false) {
                    return fresh;
                }
                guard
            }
        };
        let read = self.store.read();
        let now = self.store.clock.now_millis();
        let mut state = self.state();
        state.tried_at = now;
        match read {
            Ok(fresh) => {
                // ALLOC: one record a lease TTL.
                state.secrets = Some(Arc::new(fresh));
                state.due_at = now.saturating_add(self.ttl_millis);
            }
            Err(_) => {
                state.due_at = now.saturating_add(RETRY_MILLIS.min(self.ttl_millis));
            }
        }
        state.secrets.clone()
    }
}

impl SecretSource for CachedTransportSecrets {
    fn secrets(&self) -> Option<Arc<Secrets>> {
        self.get(false)
    }

    fn refresh(&self) {
        self.get(true);
    }
}

/// The value as the authenticator holds it, at the register's generation
/// (the Java's `atGeneration`, which reads it as a signed long).
fn secrets(value: TransportSecret, generation: u64) -> Secrets {
    Secrets {
        generation: generation.cast_signed(),
        current: value.current,
        previous: value.previous,
        rotated_at_millis: value.rotated_at_millis,
    }
}

/// A fresh key: 32 random bytes as lowercase hex, as the Java's `freshHex`.
fn fresh_key() -> Result<String, ClusterError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| ClusterError::TransportSecret("no system randomness for a new key"))?;
    Ok(lower_hex(&bytes))
}

/// `MessageDigests.toHexString`: two lowercase digits a byte.
fn lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    // ALLOC: two characters a byte, 64 for a key, once a deployment.
    let mut key = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        for nibble in [b >> 4, b & 0xf] {
            key.push(char::from(
                DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'0'),
            ));
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::lower_hex;

    #[test]
    fn a_key_is_each_bytes_two_lowercase_digits_high_first() {
        assert_eq!(lower_hex(&[0x00, 0x0f, 0xf0, 0xa5, 0xff]), "000ff0a5ff");
    }
}
