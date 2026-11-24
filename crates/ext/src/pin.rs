//! An index's analyzer pins, as a create request gives them and as they are
//! written into the descriptor's settings
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)).
//!
//! A pin is three settings under the key the Java reads for a custom
//! analyzer, `index.analysis.analyzer.<name>`: `type` (`serverless_module`,
//! a type the Java does not have), `module` (the module's SHA-256) and
//! `version` (the point's major version). The coordinator adds the resolved
//! limits beside them; a client may not.

use std::fmt;

/// The most pins an index may have.
pub const MAX_PINS: usize = 4;
/// The most bytes of analysis settings (keys and values) a create may carry.
pub const MAX_PIN_SETTINGS_BYTES: usize = 16 * 1024;
/// The prefix of every pin's name; it keeps a pin from colliding with a
/// built-in analyzer or the Java's `default` key.
pub const NAME_PREFIX: &str = "ext_";
/// The longest part of a name after the prefix.
const MAX_NAME_TAIL: usize = 60;
/// The type a pin's `type` setting must have.
pub const PIN_TYPE: &str = "serverless_module";
/// The only point version this host accepts.
pub const POINT_VERSION: u32 = 1;

const ANALYSIS: &str = "index.analysis.";
const ANALYZER: &str = "index.analysis.analyzer.";

/// An analyzer pin: a name, the module's hash and the point version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// The analyzer's name, as a mapping says it.
    pub name: String,
    /// The module's SHA-256, 64 lowercase hex digits.
    pub module: String,
    /// The point's major version.
    pub version: u32,
}

/// The limits a pin's calls run under, resolved at creation and never read
/// from live configuration again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedLimits {
    /// Instructions a call may run.
    pub fuel: u64,
    /// Bytes of linear memory.
    pub memory_bytes: u64,
    /// Milliseconds a call may take.
    pub deadline_ms: u64,
}

/// A pin setting that is refused, naming the key and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinError {
    key: String,
    reason: String,
}

impl PinError {
    fn new(key: &str, reason: impl Into<String>) -> Self {
        Self {
            // ALLOC: one refusal, a setting key of the request.
            key: key.to_owned(),
            reason: reason.into(),
        }
    }

    /// The setting key at fault.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Why it is refused.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for PinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]: {}", self.key, self.reason)
    }
}

impl std::error::Error for PinError {}

/// Whether `name` is a pin's name: the prefix and 1 to 60 of `[a-z0-9_]`.
pub fn is_pin_name(name: &str) -> bool {
    name.strip_prefix(NAME_PREFIX).is_some_and(|tail| {
        !tail.is_empty()
            && tail.len() <= MAX_NAME_TAIL
            && tail
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
    })
}

fn is_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Default)]
struct Partial {
    name: String,
    kind: Option<String>,
    module: Option<String>,
    version: Option<String>,
}

/// The pins in a create's flattened settings. Any `index.analysis.*` key
/// that is not a pin's `type`, `module` or `version` is refused, including
/// `limits.*`, which only the coordinator writes.
///
/// # Errors
///
/// A [`PinError`] naming the first key at fault: a name without the
/// prefix, a missing or null part, a type other than `serverless_module`, a
/// hash that is not 64 lowercase hex digits, a version other than 1, more
/// than [`MAX_PINS`] pins, or more than [`MAX_PIN_SETTINGS_BYTES`] of
/// analysis settings.
pub fn parse_pins(settings: &[(String, Option<String>)]) -> Result<Vec<Pin>, PinError> {
    let mut bytes = 0_usize;
    // ALLOC: one entry a pin, at most MAX_PINS before the count is refused.
    let mut found: Vec<Partial> = Vec::new();
    for (key, value) in settings {
        if !key.starts_with(ANALYSIS) {
            continue;
        }
        bytes = bytes
            .saturating_add(key.len())
            .saturating_add(value.as_deref().map_or(0, str::len));
        if bytes > MAX_PIN_SETTINGS_BYTES {
            return Err(PinError::new(key, "the analysis settings are over 16 KiB"));
        }
        let rest = key
            .strip_prefix(ANALYZER)
            .ok_or_else(|| PinError::new(key, "only analyzer pins can be set at create"))?;
        let (name, part) = rest
            .split_once('.')
            .ok_or_else(|| PinError::new(key, "not a pin setting"))?;
        if !is_pin_name(name) {
            return Err(PinError::new(
                key,
                "a pin's name is `ext_` and 1 to 60 of a-z, 0-9 and _",
            ));
        }
        let at = match found.iter().position(|p| p.name == name) {
            Some(i) => i,
            None => {
                if found.len() >= MAX_PINS {
                    return Err(PinError::new(key, "at most 4 pins an index"));
                }
                // ALLOC: one pin's name.
                found.push(Partial {
                    name: name.to_owned(),
                    ..Partial::default()
                });
                found.len().saturating_sub(1)
            }
        };
        let slot = found
            .get_mut(at)
            .ok_or_else(|| PinError::new(key, "not a pin setting"))?;
        let target = match part {
            "type" => &mut slot.kind,
            "module" => &mut slot.module,
            "version" => &mut slot.version,
            _ => return Err(PinError::new(key, "not a setting a create may carry")),
        };
        // ALLOC: one value, inside the 16 KiB bound.
        *target = value.clone();
    }
    found.into_iter().map(finish).collect()
}

fn finish(p: Partial) -> Result<Pin, PinError> {
    let key = |part: &str| format!("{ANALYZER}{}.{part}", p.name);
    let kind = p
        .kind
        .ok_or_else(|| PinError::new(&key("type"), "a pin needs its type"))?;
    if kind != PIN_TYPE {
        return Err(PinError::new(
            &key("type"),
            format!("the type of a pin is {PIN_TYPE}"),
        ));
    }
    let module = p
        .module
        .ok_or_else(|| PinError::new(&key("module"), "a pin needs its module"))?;
    if !is_hash(&module) {
        return Err(PinError::new(
            &key("module"),
            "the module is a SHA-256: 64 lowercase hex digits",
        ));
    }
    let version = p
        .version
        .ok_or_else(|| PinError::new(&key("version"), "a pin needs its version"))?;
    if version.parse::<u32>().ok() != Some(POINT_VERSION) {
        return Err(PinError::new(
            &key("version"),
            "the point version must be 1",
        ));
    }
    Ok(Pin {
        name: p.name,
        module,
        version: POINT_VERSION,
    })
}

/// The settings a descriptor carries for `pins`: the three keys of each and
/// its resolved limits, in a fixed order.
pub fn render(pins: &[(Pin, ResolvedLimits)]) -> Vec<(String, String)> {
    // ALLOC: six entries a pin, at most MAX_PINS pins.
    let mut out = Vec::with_capacity(pins.len().saturating_mul(6));
    for (pin, limits) in pins {
        let p = format!("{ANALYZER}{}", pin.name);
        out.push((format!("{p}.type"), PIN_TYPE.to_owned()));
        out.push((format!("{p}.module"), pin.module.clone()));
        out.push((format!("{p}.version"), pin.version.to_string()));
        out.push((format!("{p}.limits.fuel"), limits.fuel.to_string()));
        out.push((
            format!("{p}.limits.memory_bytes"),
            limits.memory_bytes.to_string(),
        ));
        out.push((
            format!("{p}.limits.deadline_ms"),
            limits.deadline_ms.to_string(),
        ));
    }
    out
}
