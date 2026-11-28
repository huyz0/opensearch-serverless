//! A node's lease: the register that says a node is alive, what it does,
//! and which epochs of its write log a successor must replay.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/membership/NodeLease.java`
//! (`toBytes`, `fromStream`) and `store/NodeLogEpoch.java`. The Java writes
//! compact JSON with `XContentBuilder`: the five required fields, then each
//! optional one only when set, so a lease from a node that does not publish
//! a field is byte-identical to one written before the field existed. Roles
//! are written sorted and without repeats, a blank zone is dropped, and a
//! `node_log` that is empty is left out. Unknown fields are skipped, for a
//! lease written by a newer node.
//!
//! Where the Java coerces a JSON type it never writes there (a number as
//! text, `null` as absent), or keeps the last of a repeated field, this
//! refuses: such a lease came from no shell node (`fixtures/node-leases/strict.txt`).

use std::cmp::Ordering;

use crate::error::FormatError;
use crate::json::out::Out;
use crate::json::{Reader, Token};

const WHAT: &str = "node lease";

/// The longest lease read or written: room for the required fields, every
/// optional one and about twenty node-log epochs. A lease a Java node wrote
/// longer than this is refused, which a refresh treats as not live and a
/// liveness check as alive.
pub const MAX_LEASE_BYTES: usize = 2048;

/// One epoch of a node's write log that still holds records not yet
/// published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeLogEpoch {
    /// The epoch; never negative.
    pub epoch: i64,
    /// Records before this have been published; never negative.
    pub trim: i64,
    /// A hint of how far records have landed; never negative.
    pub landed: i64,
}

/// A node's lease, as the Java's `NodeLease` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeLease {
    /// The stable node identity.
    pub node_id: String,
    /// Changes on every restart, so a restarted node is told apart.
    pub ephemeral_id: String,
    /// The transport address, a hint for peers.
    pub address: String,
    /// What the node accepts, sorted as the Java writes them (by UTF-16
    /// code unit) and without repeats: keep them so with [`NodeLease::set_roles`].
    roles: Vec<String>,
    /// The expiry the writer stamped, epoch milliseconds.
    pub expires_at_millis: i64,
    /// The node's name, if it publishes one (it may be blank).
    pub name: Option<String>,
    /// The node's build version, if it publishes one.
    pub version: Option<String>,
    /// Whether a successor revoked the lease.
    pub revoked: bool,
    /// Shards held, or -1 when the lease does not say.
    pub held: i32,
    /// The most shards the node holds, or -1.
    pub cap: i32,
    /// Shards used in the last minute, or -1.
    pub in_use: i32,
    /// Shards turned away at the cap in the last minute, or -1.
    pub refused: i32,
    /// The failure domain the node runs in, never blank.
    zone: Option<String>,
    /// When the node first advertised the `search` role, epoch milliseconds;
    /// `None` when the lease does not say (a fully ramped reader).
    pub search_since: Option<i64>,
    /// How warm the node judges its cache, 0 to 1000; `None` when it does not say.
    ramp_permille: Option<i32>,
    /// The write-log epochs a successor must fence and replay, oldest first.
    pub node_log: Vec<NodeLogEpoch>,
}

/// The Java's `String.compareTo`: by UTF-16 code unit, which differs from
/// byte order only between characters above U+FFFF and those from U+E000.
pub fn java_order(a: &str, b: &str) -> Ordering {
    // Equal strings, the common case for keys of one index, compare as bytes.
    if a == b {
        return Ordering::Equal;
    }
    a.encode_utf16().cmp(b.encode_utf16())
}

impl NodeLease {
    /// A lease with the required fields and nothing optional.
    pub fn new(node_id: &str, ephemeral_id: &str, address: &str, expires_at_millis: i64) -> Self {
        Self {
            // ALLOC: one lease's three ids, bounded by MAX_LEASE_BYTES when written.
            node_id: node_id.to_owned(),
            // ALLOC: as above.
            ephemeral_id: ephemeral_id.to_owned(),
            // ALLOC: as above.
            address: address.to_owned(),
            // ALLOC: none until roles are set.
            roles: Vec::new(),
            expires_at_millis,
            name: None,
            version: None,
            revoked: false,
            held: -1,
            cap: -1,
            in_use: -1,
            refused: -1,
            zone: None,
            search_since: None,
            ramp_permille: None,
            // ALLOC: none until epochs are set.
            node_log: Vec::new(),
        }
    }

    /// The roles, sorted and without repeats.
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// Sets the roles, sorted as the Java writes them and without repeats.
    pub fn set_roles<I: IntoIterator<Item = String>>(&mut self, roles: I) {
        // ALLOC: the node's roles, a handful.
        let mut roles: Vec<String> = roles.into_iter().collect();
        roles.sort_by(|a, b| java_order(a, b));
        roles.dedup();
        self.roles = roles;
    }

    /// The zone, `None` when unknown.
    pub fn zone(&self) -> Option<&str> {
        self.zone.as_deref()
    }

    /// The node's own judgement of how warm it is, 0 to 1000, `None` when unknown.
    pub fn ramp_permille(&self) -> Option<i32> {
        self.ramp_permille
    }

    /// Sets the ramp; a value outside 0 to 1000 is clamped into it.
    pub fn set_ramp_permille(&mut self, ramp: Option<i32>) {
        self.ramp_permille = ramp.map(|r| r.clamp(0, 1000));
    }

    /// Sets the zone; a blank one is unknown (`withZone`).
    pub fn set_zone(&mut self, zone: Option<String>) {
        self.zone = zone.filter(|z| !z.trim().is_empty());
    }

    /// The name to show: the node id when none, or a blank one, is published.
    pub fn display_name(&self) -> &str {
        match &self.name {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.node_id,
        }
    }

    /// Whether the lease has run out by `now_millis`, the observer's clock:
    /// a revoked lease is dead whatever its expiry says.
    pub fn is_expired_at(&self, now_millis: i64) -> bool {
        self.revoked || now_millis >= self.expires_at_millis
    }

    /// Writes the lease as the Java's `toBytes`, into the front of `buf`,
    /// and returns its length.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when the lease is longer than `buf` or than
    /// [`MAX_LEASE_BYTES`]: `actual` is the length it needs.
    pub fn write(&self, buf: &mut [u8]) -> Result<usize, FormatError> {
        let cap = buf.len().min(MAX_LEASE_BYTES);
        let mut out = Out::new(buf.get_mut(..cap).unwrap_or_default());
        out.start_object();
        out.string_field("node_id", &self.node_id);
        out.string_field("ephemeral_id", &self.ephemeral_id);
        out.string_field("address", &self.address);
        out.start_array_field("roles");
        for role in &self.roles {
            out.string(role);
        }
        out.end_array();
        out.int_field("expires_at_millis", self.expires_at_millis);
        if let Some(name) = &self.name {
            out.string_field("name", name);
        }
        if let Some(version) = &self.version {
            out.string_field("version", version);
        }
        if self.revoked {
            out.bool_field("revoked", true);
        }
        for (field, n) in [
            ("held", self.held),
            ("cap", self.cap),
            ("in_use", self.in_use),
            ("refused", self.refused),
        ] {
            if n >= 0 {
                out.int_field(field, i64::from(n));
            }
        }
        if let Some(zone) = &self.zone {
            out.string_field("zone", zone);
        }
        if let Some(since) = self.search_since {
            out.int_field("search_since", since);
        }
        if let Some(ramp) = self.ramp_permille {
            out.int_field("ramp_permille", i64::from(ramp));
        }
        if !self.node_log.is_empty() {
            out.start_array_field("node_log");
            for epoch in &self.node_log {
                out.start_object();
                out.int_field("epoch", epoch.epoch);
                out.int_field("trim", epoch.trim);
                out.int_field("landed", epoch.landed);
                out.end_object();
            }
            out.end_array();
        }
        out.end_object();
        out.finish(WHAT)
    }

    /// Parses a lease as the Java's `fromStream` does.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] past [`MAX_LEASE_BYTES`];
    /// [`FormatError::Corrupt`] for malformed JSON, content after the lease,
    /// a missing `node_id`, `ephemeral_id` or `address`, a node-log entry
    /// without an epoch or with a negative number, a repeated field, or a
    /// field of a type the Java never writes there.
    pub fn parse(value: &[u8]) -> Result<Self, FormatError> {
        if value.len() > MAX_LEASE_BYTES {
            return Err(FormatError::TooLarge {
                what: WHAT,
                limit: MAX_LEASE_BYTES as u64,
                actual: u64::try_from(value.len()).unwrap_or(u64::MAX),
            });
        }
        let mut json = Reader::new(value, WHAT);
        json.start_object("a lease is an object")?;
        let mut fields = Fields::new();
        while let Some(field) = json.next_field()? {
            fields.read(&mut json, &field)?;
        }
        json.finish()?;
        fields.lease(&json)
    }
}

/// The fields read so far, each at most once.
struct Fields {
    seen: u16,
    node_id: Option<String>,
    ephemeral_id: Option<String>,
    address: Option<String>,
    roles: Vec<String>,
    expires: i64,
    name: Option<String>,
    version: Option<String>,
    revoked: bool,
    counts: [i32; 4],
    zone: Option<String>,
    search_since: Option<i64>,
    ramp: Option<i64>,
    node_log: Vec<NodeLogEpoch>,
}

const FIELDS: [&str; 16] = [
    "node_id",
    "ephemeral_id",
    "address",
    "roles",
    "expires_at_millis",
    "name",
    "version",
    "revoked",
    "held",
    "cap",
    "in_use",
    "refused",
    "zone",
    "node_log",
    "search_since",
    "ramp_permille",
];

impl Fields {
    fn new() -> Self {
        Self {
            seen: 0,
            node_id: None,
            ephemeral_id: None,
            address: None,
            // ALLOC: none until roles are read.
            roles: Vec::new(),
            expires: 0,
            name: None,
            version: None,
            revoked: false,
            // Absent shard counts are -1, as the Java's defaults.
            counts: [-1; 4],
            zone: None,
            search_since: None,
            ramp: None,
            // ALLOC: none until epochs are read.
            node_log: Vec::new(),
        }
    }

    fn read(&mut self, json: &mut Reader<'_>, field: &str) -> Result<(), FormatError> {
        let Some(at) = FIELDS.iter().position(|f| *f == field) else {
            return json.skip_value();
        };
        let bit = 1u16 << at;
        if self.seen & bit != 0 {
            return Err(json.error("a lease field is repeated"));
        }
        self.seen |= bit;
        match at {
            0 => self.node_id = Some(text(json)?),
            1 => self.ephemeral_id = Some(text(json)?),
            2 => self.address = Some(text(json)?),
            3 => self.roles = roles(json)?,
            4 => self.expires = int(json)?,
            5 => self.name = Some(text(json)?),
            6 => self.version = Some(text(json)?),
            7 => {
                let Token::Bool(revoked) = json.expect_token()? else {
                    return Err(json.error("revoked is a boolean"));
                };
                self.revoked = revoked;
            }
            8..=11 => {
                let n = i32::try_from(int(json)?)
                    .map_err(|_| json.error("a lease's shard count is an int"))?;
                if let Some(slot) = self.counts.get_mut(at.saturating_sub(8)) {
                    *slot = n;
                }
            }
            12 => self.zone = Some(text(json)?),
            13 => self.node_log = epochs(json)?,
            14 => self.search_since = Some(int(json)?),
            _ => self.ramp = Some(int(json)?),
        }
        Ok(())
    }

    fn lease(self, json: &Reader<'_>) -> Result<NodeLease, FormatError> {
        let (Some(node_id), Some(ephemeral_id), Some(address)) =
            (self.node_id, self.ephemeral_id, self.address)
        else {
            return Err(json.error("a lease needs node_id, ephemeral_id and address"));
        };
        let [held, cap, in_use, refused] = self.counts.map(|n| if n < 0 { -1 } else { n });
        let mut lease = NodeLease {
            node_id,
            ephemeral_id,
            address,
            // ALLOC: none; set below.
            roles: Vec::new(),
            expires_at_millis: self.expires,
            name: self.name,
            version: self.version,
            revoked: self.revoked,
            held,
            cap,
            in_use,
            refused,
            zone: None,
            search_since: self.search_since,
            ramp_permille: None,
            node_log: self.node_log,
        };
        lease.set_ramp_permille(
            self.ramp
                .map(|r| i32::try_from(r.clamp(0, 1000)).unwrap_or(0)),
        );
        lease.set_roles(self.roles);
        lease.set_zone(self.zone);
        Ok(lease)
    }
}

fn text(json: &mut Reader<'_>) -> Result<String, FormatError> {
    match json.expect_token()? {
        Token::Str(s) => Ok(s.into_owned()),
        _ => Err(json.error("a lease's text field is a string")),
    }
}

fn int(json: &mut Reader<'_>) -> Result<i64, FormatError> {
    match json.expect_token()? {
        Token::Int(n) => Ok(n),
        _ => Err(json.error("a lease's number is an integer")),
    }
}

fn roles(json: &mut Reader<'_>) -> Result<Vec<String>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("roles is an array"));
    }
    // ALLOC: the lease's roles, bounded by MAX_LEASE_BYTES.
    let mut roles = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(roles),
            Token::Str(s) => roles.push(s.into_owned()),
            _ => return Err(json.error("a role is a string")),
        }
    }
}

fn epochs(json: &mut Reader<'_>) -> Result<Vec<NodeLogEpoch>, FormatError> {
    if json.expect_token()? != Token::StartArray {
        return Err(json.error("node_log is an array"));
    }
    // ALLOC: the lease's epochs, bounded by MAX_LEASE_BYTES.
    let mut epochs = Vec::new();
    loop {
        match json.expect_token()? {
            Token::EndArray => return Ok(epochs),
            Token::StartObject => epochs.push(epoch(json)?),
            _ => return Err(json.error("a node_log entry is an object")),
        }
    }
}

fn epoch(json: &mut Reader<'_>) -> Result<NodeLogEpoch, FormatError> {
    let mut found: [Option<i64>; 3] = [None; 3];
    while let Some(field) = json.next_field()? {
        let at = match field.as_ref() {
            "epoch" => 0,
            "trim" => 1,
            "landed" => 2,
            _ => {
                json.skip_value()?;
                continue;
            }
        };
        let n = int(json)?;
        if let Some(slot) = found.get_mut(at)
            && slot.replace(n).is_some()
        {
            return Err(json.error("a node_log field is repeated"));
        }
    }
    let [epoch, trim, landed] = found;
    let Some(epoch) = epoch else {
        return Err(json.error("a node_log entry has no epoch"));
    };
    let (trim, landed) = (trim.unwrap_or(0), landed.unwrap_or(0));
    if epoch < 0 || trim < 0 || landed < 0 {
        return Err(json.error("a node_log entry's numbers are not negative"));
    }
    Ok(NodeLogEpoch {
        epoch,
        trim,
        landed,
    })
}
