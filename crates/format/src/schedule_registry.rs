//! The schedule registry shared by every plugin that runs work on a timer:
//! which tenants have enabled work, and which slot of an item last ran.
//!
//! A node cannot list a bucket, so the tenants a scheduler must visit are
//! named in 256 shard registers, `<namespace>/registry/<n>`. A shard's value
//! is its tenant ids in byte order, one a line. An item's run claim is a
//! register `<namespace>/runs/<tenant>/<item>` holding the last claimed slot
//! as eight big-endian bytes. The namespace (`alerting`, `ad`) keeps one
//! plugin's registers apart from another's; the plain functions here are the
//! alerting namespace.

use crate::error::{FormatError, corrupt};

const WHAT: &str = "schedule registry shard";

/// The namespace of the alerting plugin, whose keys predate the others.
pub const ALERTING_NAMESPACE: &str = "alerting";

/// How many shard registers the registry is spread over.
pub const REGISTRY_SHARDS: usize = 256;

/// The most tenants one shard names.
pub const MAX_REGISTRY_ENTRIES: usize = 4096;

/// The longest tenant id, in UTF-8 bytes.
pub const MAX_TENANT_ID_BYTES: usize = 64;

/// The longest shard value: [`MAX_REGISTRY_ENTRIES`] ids of the longest
/// length and a newline each.
pub const MAX_REGISTRY_SHARD_BYTES: usize = MAX_REGISTRY_ENTRIES * (MAX_TENANT_ID_BYTES + 1);

/// Bytes of a run claim's value.
pub const RUN_CLAIM_BYTES: usize = 8;

/// Refuses a tenant id that is empty, too long, a dot segment, or holds a
/// `/` or a control character (a newline ends a registry line).
///
/// # Errors
///
/// [`FormatError::Corrupt`] or [`FormatError::TooLarge`].
pub fn check_tenant_id(tenant: &str) -> Result<(), FormatError> {
    check_segment(tenant, "a tenant id")
}

fn check_segment(id: &str, what: &'static str) -> Result<(), FormatError> {
    if id.len() > MAX_TENANT_ID_BYTES {
        return Err(FormatError::TooLarge {
            what,
            limit: MAX_TENANT_ID_BYTES as u64,
            actual: id.len() as u64,
        });
    }
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.chars().any(char::is_control)
    {
        return Err(corrupt(
            what,
            "an id is a plain path segment of printable characters",
        ));
    }
    Ok(())
}

/// The shard a tenant is named in: FNV-1a of its bytes, modulo
/// [`REGISTRY_SHARDS`].
pub fn registry_shard_of(tenant: &str) -> usize {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in tenant.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash as usize % REGISTRY_SHARDS
}

fn check_base(base: &str) -> Result<(), FormatError> {
    if base.is_empty() || base.ends_with('/') {
        Ok(())
    } else {
        Err(corrupt(WHAT, "a base is empty or ends in '/'"))
    }
}

fn check_namespace(namespace: &str) -> Result<(), FormatError> {
    check_segment(namespace, "a namespace")
}

/// The key of a shard's register in the alerting namespace.
///
/// # Errors
///
/// As [`registry_shard_key_in`].
pub fn registry_shard_key(base: &str, shard: usize) -> Result<String, FormatError> {
    registry_shard_key_in(base, ALERTING_NAMESPACE, shard)
}

/// The key of a shard's register.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a base that is not empty and does not end in
/// `/`, a namespace that is not a plain path segment, or a shard past
/// [`REGISTRY_SHARDS`].
pub fn registry_shard_key_in(
    base: &str,
    namespace: &str,
    shard: usize,
) -> Result<String, FormatError> {
    check_base(base)?;
    check_namespace(namespace)?;
    if shard >= REGISTRY_SHARDS {
        return Err(corrupt(WHAT, "a shard is below the shard count"));
    }
    // ALLOC: one key, the length of its parts.
    Ok(format!("{base}{namespace}/registry/{shard:03}"))
}

/// The key of a monitor's run claim register in the alerting namespace.
///
/// # Errors
///
/// As [`run_claim_key_in`].
pub fn run_claim_key(base: &str, tenant: &str, monitor: &str) -> Result<String, FormatError> {
    run_claim_key_in(base, ALERTING_NAMESPACE, tenant, monitor)
}

/// The key of an item's run claim register.
///
/// # Errors
///
/// As [`registry_shard_key_in`], and for a tenant or item id that is not a
/// plain path segment.
pub fn run_claim_key_in(
    base: &str,
    namespace: &str,
    tenant: &str,
    item: &str,
) -> Result<String, FormatError> {
    check_base(base)?;
    check_namespace(namespace)?;
    check_tenant_id(tenant)?;
    check_segment(item, "an item id")?;
    // ALLOC: one key, the length of its parts.
    Ok([base, namespace, "/runs/", tenant, "/", item].concat())
}

/// A shard's tenants, in byte order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegistryShard {
    entries: Vec<String>,
}

impl RegistryShard {
    /// An empty shard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a shard's value.
    ///
    /// # Errors
    ///
    /// [`FormatError::Corrupt`] for ids that are not in order, repeat, or are
    /// invalid; [`FormatError::TooLarge`] past the bounds.
    pub fn parse(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() > MAX_REGISTRY_SHARD_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_REGISTRY_SHARD_BYTES as u64,
                actual: bytes.len() as u64,
            });
        }
        let text = std::str::from_utf8(bytes).map_err(|_| corrupt(WHAT, "text is UTF-8"))?;
        // ALLOC: ids, at most MAX_REGISTRY_ENTRIES.
        let mut entries: Vec<String> = Vec::new();
        if !text.is_empty() {
            let body = text
                .strip_suffix('\n')
                .ok_or_else(|| corrupt(WHAT, "every id ends in a newline"))?;
            for id in body.split('\n') {
                check_tenant_id(id)?;
                if entries.last().is_some_and(|last| last.as_str() >= id) {
                    return Err(corrupt(WHAT, "ids are in byte order, each once"));
                }
                if entries.len() >= MAX_REGISTRY_ENTRIES {
                    return Err(FormatError::TooLarge {
                        what: WHAT,
                        limit: MAX_REGISTRY_ENTRIES as u64,
                        actual: (entries.len() as u64).saturating_add(1),
                    });
                }
                // ALLOC: one id each, at most MAX_REGISTRY_ENTRIES.
                entries.push(id.to_owned());
            }
        }
        Ok(Self { entries })
    }

    /// Writes the value into the front of `out`: its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when `out` is short.
    pub fn write(&self, out: &mut [u8]) -> Result<usize, FormatError> {
        let mut at = 0usize;
        for id in &self.entries {
            let end = at.saturating_add(id.len()).saturating_add(1);
            let Some(slot) = out.get_mut(at..end) else {
                return Err(FormatError::TooLarge {
                    what: WHAT,
                    limit: out.len() as u64,
                    actual: end as u64,
                });
            };
            let (text, newline) = slot.split_at_mut(id.len());
            text.copy_from_slice(id.as_bytes());
            newline.fill(b'\n');
            at = end;
        }
        Ok(at)
    }

    /// Whether the tenant is named.
    pub fn contains(&self, tenant: &str) -> bool {
        self.position(tenant).is_ok()
    }

    fn position(&self, tenant: &str) -> Result<usize, usize> {
        self.entries.binary_search_by(|e| e.as_str().cmp(tenant))
    }

    /// Names the tenant: whether it was added (false when already there).
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when a new tenant would pass
    /// [`MAX_REGISTRY_ENTRIES`]; [`FormatError::Corrupt`] for an invalid id.
    pub fn insert(&mut self, tenant: &str) -> Result<bool, FormatError> {
        check_tenant_id(tenant)?;
        let Err(at) = self.position(tenant) else {
            return Ok(false);
        };
        if self.entries.len() >= MAX_REGISTRY_ENTRIES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_REGISTRY_ENTRIES as u64,
                actual: (self.entries.len() as u64).saturating_add(1),
            });
        }
        // ALLOC: one id, at most MAX_REGISTRY_ENTRIES kept.
        self.entries.insert(at, tenant.to_owned());
        Ok(true)
    }

    /// Removes the tenant: whether it was there.
    pub fn remove(&mut self, tenant: &str) -> bool {
        match self.position(tenant) {
            Ok(at) => {
                self.entries.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// The tenants, in byte order.
    pub fn tenants(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    /// How many tenants.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no tenant is named.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A run claim's value.
pub fn encode_run_claim(slot: u64) -> [u8; RUN_CLAIM_BYTES] {
    slot.to_be_bytes()
}

/// Reads a run claim's value.
///
/// # Errors
///
/// [`FormatError::Corrupt`] for a value that is not eight bytes.
pub fn decode_run_claim(bytes: &[u8]) -> Result<u64, FormatError> {
    let raw: [u8; RUN_CLAIM_BYTES] = bytes
        .try_into()
        .map_err(|_| corrupt("schedule run claim", "a claim is eight bytes"))?;
    Ok(u64::from_be_bytes(raw))
}
