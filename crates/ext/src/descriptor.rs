//! The strict parse of what a module says about itself.
//!
//! Exact keys, no duplicates, every value checked before it is kept, and
//! every refusal names the key. Nothing is allocated by a size the module
//! reports: the input is at most [`MAX_DESCRIPTOR_BYTES`].

use std::fmt;

use crate::json::{self, Fail, Value};
use crate::{MAX_DESCRIPTOR_BYTES, MAX_IO_BYTES};

const MAX_POINTS: usize = 64;
const MAX_CONCURRENT: u64 = 1024;

/// Why a descriptor was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorError {
    key: String,
    reason: &'static str,
}

impl DescriptorError {
    pub(crate) fn at(key: &str, reason: &'static str) -> Self {
        Self {
            // ALLOC: one refusal, a key of the descriptor, cut and cleaned as a module's text is.
            key: crate::error::clean(key.as_bytes()),
            reason,
        }
    }

    /// The key at fault, `points[1].id` for a nested one, empty when the
    /// whole document is.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// What is wrong with it.
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "descriptor: {}", self.reason)
        } else {
            write!(f, "descriptor key `{}`: {}", self.key, self.reason)
        }
    }
}

impl std::error::Error for DescriptorError {}

/// Limits a module may ask for; each can only lower the host's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Instructions a call may run.
    pub fuel: Option<u64>,
    /// Bytes of linear memory.
    pub memory_bytes: Option<u64>,
    /// Milliseconds a call may take.
    pub deadline_ms: Option<u64>,
}

/// One extension point a module serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    id: String,
    version: (u32, u32, u32),
}

impl Point {
    /// The point's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The point's version.
    pub fn version(&self) -> (u32, u32, u32) {
        self.version
    }
}

/// A module's descriptor, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    name: String,
    version: (u32, u32, u32),
    abi: (u16, u16),
    io_bytes: u32,
    max_concurrent_calls: u32,
    limits: Limits,
    points: Vec<Point>,
    plugin: Option<Vec<(String, Value)>>,
}

impl Descriptor {
    /// Parses and validates `bytes`. A caller judges the module's ABI version
    /// ([`crate::abi_accepted`]) first, so a module built for a newer ABI is
    /// refused for that and not for a key this host does not know.
    ///
    /// # Errors
    ///
    /// [`DescriptorError`] naming the first key at fault, or none for a
    /// document that is too large, not UTF-8 or not JSON.
    pub fn parse(bytes: &[u8]) -> Result<Self, DescriptorError> {
        if bytes.len() > MAX_DESCRIPTOR_BYTES as usize {
            return Err(DescriptorError::at(
                "",
                "larger than the descriptor ceiling",
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| DescriptorError::at("", "not UTF-8"))?;
        let root = match json::parse(text) {
            Ok(v) => v,
            Err(Fail::Duplicate(k)) => return Err(DescriptorError::at(&k, "repeated")),
            Err(Fail::Syntax) => return Err(DescriptorError::at("", "not JSON")),
        };
        let Value::Obj(members) = root else {
            return Err(DescriptorError::at("", "not an object"));
        };
        let members = Members::new(
            "",
            members,
            &[
                "name",
                "version",
                "abi",
                "io_bytes",
                "max_concurrent_calls",
                "limits",
                "points",
                "plugin",
            ],
        )?;
        let plugin = match members.get("plugin") {
            None => None,
            Some(Value::Obj(section)) => Some(section.clone()),
            Some(_) => return Err(DescriptorError::at("plugin", "not an object")),
        };
        let (abi_major, abi_minor) = pair(&members.string("abi")?, "abi")?;
        let io_bytes = members.int("io_bytes", 1, u64::from(MAX_IO_BYTES))?;
        let concurrent = members.int("max_concurrent_calls", 1, MAX_CONCURRENT)?;
        Ok(Self {
            name: name(&members.string("name")?, "name", true)?,
            version: triple(&members.string("version")?, "version")?,
            abi: (abi_major, abi_minor),
            io_bytes: u32::try_from(io_bytes).unwrap_or(u32::MAX),
            max_concurrent_calls: u32::try_from(concurrent).unwrap_or(u32::MAX),
            limits: limits(&members)?,
            points: points(&members)?,
            plugin,
        })
    }

    /// The `plugin` section, when the module declares one.
    pub(crate) fn plugin_section(&self) -> Option<&[(String, Value)]> {
        self.plugin.as_deref()
    }

    /// The module's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The module's own version.
    pub fn version(&self) -> (u32, u32, u32) {
        self.version
    }

    /// The ABI `(major, minor)` it was built against.
    pub fn abi(&self) -> (u16, u16) {
        self.abi
    }

    /// The size of each I/O region.
    pub fn io_bytes(&self) -> u32 {
        self.io_bytes
    }

    /// The most calls the host may run at once.
    pub fn max_concurrent_calls(&self) -> u32 {
        self.max_concurrent_calls
    }

    /// What the module asks to lower.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The points it serves.
    pub fn points(&self) -> &[Point] {
        &self.points
    }
}

/// The members of one object, checked against the keys allowed.
pub(crate) struct Members {
    prefix: String,
    items: Vec<(String, Value)>,
}

pub(crate) fn path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        // ALLOC: one refusal, a key of the descriptor.
        key.to_owned()
    } else {
        // ALLOC: one refusal, a key of the descriptor.
        format!("{prefix}.{key}")
    }
}

impl Members {
    pub(crate) fn new(
        prefix: &str,
        items: Vec<(String, Value)>,
        allowed: &[&str],
    ) -> Result<Self, DescriptorError> {
        if let Some((k, _)) = items.iter().find(|(k, _)| !allowed.contains(&k.as_str())) {
            return Err(DescriptorError::at(&path(prefix, k), "unknown key"));
        }
        // ALLOC: one descriptor, a short key prefix.
        Ok(Self {
            prefix: prefix.to_owned(),
            items,
        })
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub(crate) fn require(&self, key: &str) -> Result<&Value, DescriptorError> {
        self.get(key)
            .ok_or_else(|| DescriptorError::at(&path(&self.prefix, key), "missing"))
    }

    pub(crate) fn string(&self, key: &str) -> Result<String, DescriptorError> {
        match self.require(key)? {
            // ALLOC: one descriptor, each string at most MAX_DESCRIPTOR_BYTES.
            Value::Str(s) => Ok(s.clone()),
            _ => Err(DescriptorError::at(
                &path(&self.prefix, key),
                "not a string",
            )),
        }
    }

    pub(crate) fn int(&self, key: &str, min: u64, max: u64) -> Result<u64, DescriptorError> {
        let at = path(&self.prefix, key);
        match self.require(key)? {
            Value::Int(n) if (min..=max).contains(n) => Ok(*n),
            Value::Int(_) => Err(DescriptorError::at(&at, "out of range")),
            _ => Err(DescriptorError::at(&at, "not a whole number")),
        }
    }
}

/// `x.y` as two numbers.
pub(crate) fn pair(text: &str, key: &str) -> Result<(u16, u16), DescriptorError> {
    let bad = || DescriptorError::at(key, "not `major.minor`");
    let (a, b) = text.split_once('.').ok_or_else(bad)?;
    Ok((number(a).ok_or_else(bad)?, number(b).ok_or_else(bad)?))
}

/// `x.y.z` as three numbers.
pub(crate) fn triple(text: &str, key: &str) -> Result<(u32, u32, u32), DescriptorError> {
    let bad = || DescriptorError::at(key, "not `major.minor.patch`");
    let mut parts = text.split('.');
    let (a, b, c) = (parts.next(), parts.next(), parts.next());
    if parts.next().is_some() {
        return Err(bad());
    }
    let n = |p: Option<&str>| p.and_then(number::<u32>).ok_or_else(bad);
    Ok((n(a)?, n(b)?, n(c)?))
}

/// A decimal number without a sign or a leading zero.
fn number<T: std::str::FromStr>(text: &str) -> Option<T> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text.len() == 1 || !text.starts_with('0'));
    if plain { text.parse().ok() } else { None }
}

/// A name of 1 to 64 characters, lowercase to start, then lowercase,
/// digits and `_` (and `-` when `dash`).
fn name(text: &str, key: &str, dash: bool) -> Result<String, DescriptorError> {
    let mut chars = text.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && text.len() <= 64
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || (dash && c == '-')
        });
    if ok {
        // ALLOC: one descriptor, at most 64 bytes.
        Ok(text.to_owned())
    } else {
        Err(DescriptorError::at(key, "not a valid name"))
    }
}

pub(crate) fn limits(members: &Members) -> Result<Limits, DescriptorError> {
    let Some(value) = members.get("limits") else {
        return Ok(Limits::default());
    };
    let Value::Obj(items) = value else {
        return Err(DescriptorError::at("limits", "not an object"));
    };
    // ALLOC: one descriptor, at most three members.
    let inner = Members::new(
        "limits",
        items.clone(),
        &["fuel", "memory_bytes", "deadline_ms"],
    )?;
    let one = |key: &str| -> Result<Option<u64>, DescriptorError> {
        if inner.get(key).is_none() {
            return Ok(None);
        }
        inner.int(key, 1, u64::MAX).map(Some)
    };
    Ok(Limits {
        fuel: one("fuel")?,
        memory_bytes: one("memory_bytes")?,
        deadline_ms: one("deadline_ms")?,
    })
}

fn points(members: &Members) -> Result<Vec<Point>, DescriptorError> {
    let Value::Arr(items) = members.require("points")? else {
        return Err(DescriptorError::at("points", "not an array"));
    };
    if items.is_empty() || items.len() > MAX_POINTS {
        return Err(DescriptorError::at("points", "needs 1 to 64 points"));
    }
    // ALLOC: one descriptor, at most MAX_POINTS entries.
    let mut out: Vec<Point> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        // ALLOC: one descriptor, a short key per point.
        let at = format!("points[{i}]");
        let at = at.as_str();
        let Value::Obj(fields) = item else {
            return Err(DescriptorError::at(at, "not an object"));
        };
        // ALLOC: one descriptor, two members.
        let one = Members::new(at, fields.clone(), &["id", "version"])?;
        let id = name(&one.string("id")?, &path(at, "id"), false)?;
        if out.iter().any(|p| p.id == id) {
            return Err(DescriptorError::at(&path(at, "id"), "repeated"));
        }
        let version = triple(&one.string("version")?, &path(at, "version"))?;
        out.push(Point { id, version });
    }
    Ok(out)
}
