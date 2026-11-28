//! The alerting registry: which tenants have an enabled monitor, and each
//! monitor's last claimed slot.
//!
//! Nothing lists, so a scheduler finds the tenants to visit in 256 shard
//! registers. The two protocols that keep a monitor from dropping out of its
//! shard are here and nowhere else:
//!
//! - an enabled put writes the monitor and then calls [`AlertRegistry::register`],
//!   which **always** swaps its tenant's shard (adding the entry, or
//!   rewriting it unchanged so the shard's generation moves);
//! - a removal calls [`AlertRegistry::unregister_unless`], which reads the
//!   shard, **then** asks whether the tenant still has an enabled monitor,
//!   and swaps against the generation it read.
//!
//! If a registration's swap lands after the removal's read, the removal's
//! swap loses and it asks again; if it lands before, the monitor's write came
//! before the removal's question, which then says yes. A crash of either side
//! leaves a stale entry (one empty read a TTL) and never a missing one.

use std::sync::Arc;

use oss_blob::register::ABSENT_GENERATION;
use oss_blob::{BlobStore, SwapOutcome};
use oss_format::{
    MAX_REGISTRY_SHARD_BYTES, RegistryShard, check_tenant_id, decode_run_claim, encode_run_claim,
    registry_shard_key, registry_shard_of, run_claim_key,
};

use crate::error::MetaError;

/// How many times a change reads a register and swaps before giving up to
/// contention: the spec's eight, with no wait here (a caller adds jitter).
const ATTEMPTS: u32 = 8;

/// What a claim found: whether this caller holds the slot, and the
/// register's generation to hint the next claim of the monitor with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    /// Whether this caller holds the slot.
    pub won: bool,
    /// The generation after this claim, or as last read.
    pub generation: u64,
}

/// The registry of one deployment.
pub struct AlertRegistry {
    store: Arc<dyn BlobStore>,
    base: String,
}

impl AlertRegistry {
    /// The registry under `base` (empty, or ending in `/`) of `store`.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for a base that is not empty and does not end
    /// in `/`.
    pub fn new(store: Arc<dyn BlobStore>, base: &str) -> Result<Self, MetaError> {
        registry_shard_key(base, 0)?;
        Ok(Self {
            store,
            // ALLOC: one copy of the base, once a registry.
            base: base.to_owned(),
        })
    }

    /// The tenants a shard names: one request.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn tenants(&self, shard: usize) -> Result<Vec<String>, MetaError> {
        let (read, _) = self.read(&registry_shard_key(&self.base, shard)?)?;
        // ALLOC: one id each, at most MAX_REGISTRY_ENTRIES.
        Ok(read.tenants().map(str::to_owned).collect())
    }

    /// Whether the tenant is named: one request.
    ///
    /// # Errors
    ///
    /// As [`AlertRegistry::tenants`], and for an invalid tenant id.
    pub fn is_registered(&self, tenant: &str) -> Result<bool, MetaError> {
        check_tenant_id(tenant)?;
        let key = registry_shard_key(&self.base, registry_shard_of(tenant))?;
        Ok(self.read(&key)?.0.contains(tenant))
    }

    /// Names the tenant in its shard and swaps the shard whether or not the
    /// entry was there, so a concurrent removal that read the shard earlier
    /// loses its swap. One read and one swap.
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for an invalid id, or for a new tenant in a full
    /// shard (a [`oss_format::FormatError::TooLarge`], nothing written);
    /// [`MetaError::Contended`] after eight attempts; [`MetaError::Blob`].
    pub fn register(&self, tenant: &str) -> Result<(), MetaError> {
        check_tenant_id(tenant)?;
        let key = registry_shard_key(&self.base, registry_shard_of(tenant))?;
        for _ in 0..ATTEMPTS {
            let (mut shard, generation) = self.read(&key)?;
            shard.insert(tenant)?;
            if self.swap(&key, generation, &shard)? {
                return Ok(());
            }
        }
        Err(MetaError::Contended)
    }

    /// Removes the tenant's entry unless `has_enabled` says it still has an
    /// enabled monitor. The shard is read first and `has_enabled` asked after
    /// that read, on every attempt, so an answer is never older than the
    /// generation the swap is made against. Returns whether the entry was
    /// removed (`false` when it was kept or already absent).
    ///
    /// # Errors
    ///
    /// What `has_enabled` returns; [`MetaError::Contended`] after
    /// eight attempts; [`MetaError::Format`]; [`MetaError::Blob`].
    pub fn unregister_unless(
        &self,
        tenant: &str,
        has_enabled: &mut dyn FnMut() -> Result<bool, MetaError>,
    ) -> Result<bool, MetaError> {
        check_tenant_id(tenant)?;
        let key = registry_shard_key(&self.base, registry_shard_of(tenant))?;
        for _ in 0..ATTEMPTS {
            let (mut shard, generation) = self.read(&key)?;
            if !shard.contains(tenant) || has_enabled()? {
                return Ok(false);
            }
            shard.remove(tenant);
            if self.swap(&key, generation, &shard)? {
                return Ok(true);
            }
        }
        Err(MetaError::Contended)
    }

    /// Claims the monitor's run of `slot`: raises its claim register to
    /// `slot` by compare-and-swap from a lower value. `hint` is the
    /// generation [`Claim::generation`] of this node's last claim of the
    /// monitor: with it the swap is tried first, one request when no other
    /// node claimed since; without it, or when it is stale, the register is
    /// read first (one more).
    ///
    /// # Errors
    ///
    /// [`MetaError::Format`] for an invalid id or a damaged claim;
    /// [`MetaError::Contended`] after eight attempts; [`MetaError::Blob`].
    pub fn claim(
        &self,
        tenant: &str,
        monitor: &str,
        slot: u64,
        hint: Option<u64>,
    ) -> Result<Claim, MetaError> {
        self.claim_between(tenant, monitor, slot, hint, &mut || {})
    }

    /// As [`AlertRegistry::claim`], calling `between` after each read and
    /// before the swap that follows it: the point a test moves the register.
    ///
    /// # Errors
    ///
    /// As [`AlertRegistry::claim`].
    pub fn claim_between(
        &self,
        tenant: &str,
        monitor: &str,
        slot: u64,
        hint: Option<u64>,
        between: &mut dyn FnMut(),
    ) -> Result<Claim, MetaError> {
        let key = run_claim_key(&self.base, tenant, monitor)?;
        let value = encode_run_claim(slot);
        let mut buf = [0u8; 8];
        let mut generation = hint;
        for _ in 0..ATTEMPTS {
            let from = match generation.take() {
                Some(g) => g,
                None => {
                    let (g, last) = match self.store.read_register(&key, &mut buf)? {
                        None => (ABSENT_GENERATION, None),
                        Some(read) => (
                            read.generation,
                            Some(decode_run_claim(buf.get(..read.len).unwrap_or_default())?),
                        ),
                    };
                    if last.is_some_and(|last| last >= slot) {
                        return Ok(Claim {
                            won: false,
                            generation: g,
                        });
                    }
                    between();
                    g
                }
            };
            let outcome = if from == ABSENT_GENERATION {
                self.store.create_register(&key, &value)?
            } else {
                self.store.swap_register(&key, from, &value)?
            };
            if let SwapOutcome::Applied(g) = outcome {
                return Ok(Claim {
                    won: true,
                    generation: g,
                });
            }
        }
        Err(MetaError::Contended)
    }

    fn read(&self, key: &str) -> Result<(RegistryShard, u64), MetaError> {
        // ALLOC: one shard's worth, the most a register here holds, per read.
        let mut buf = vec![0u8; MAX_REGISTRY_SHARD_BYTES];
        match self.store.read_register(key, &mut buf)? {
            None => Ok((RegistryShard::new(), ABSENT_GENERATION)),
            Some(read) => Ok((
                RegistryShard::parse(buf.get(..read.len).unwrap_or_default())?,
                read.generation,
            )),
        }
    }

    fn swap(&self, key: &str, generation: u64, shard: &RegistryShard) -> Result<bool, MetaError> {
        // ALLOC: one shard's worth, the most a register here holds, per swap.
        let mut buf = vec![0u8; MAX_REGISTRY_SHARD_BYTES];
        let len = shard.write(&mut buf)?;
        let value = buf.get(..len).unwrap_or_default();
        let outcome = if generation == ABSENT_GENERATION {
            self.store.create_register(key, value)?
        } else {
            self.store.swap_register(key, generation, value)?
        };
        Ok(matches!(outcome, SwapOutcome::Applied(_)))
    }
}
