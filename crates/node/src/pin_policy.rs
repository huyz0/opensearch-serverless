//! What a create needs to pin analyzer modules
//! ([ADR 0053](../../../docs/adr/0053-the-analyzer-extension-point.md)): who
//! may, and whether a module can be one.

use oss_ext::pin::{Pin, ResolvedLimits};

/// The node's say over a pin. A node with no policy refuses every pin.
pub trait PinPolicy: Send + Sync {
    /// Whether the user of a request (none when security is off) may pin a
    /// module; the role needed is `all_access`.
    fn authorised(&self, principal: Option<&str>) -> bool;

    /// Reads the module and says whether it can be the pin's analyzer, with
    /// the limits its calls will run under, resolved now.
    ///
    /// # Errors
    ///
    /// The words of the refusal: the module is absent, is refused by the
    /// host, is not an analyzer, imports something, or fails its probe.
    fn check(&self, pin: &Pin) -> Result<ResolvedLimits, String>;
}

/// `settings`, a JSON object's bytes, with `extra` members added as strings.
pub fn with_members(settings: &[u8], extra: &[(String, String)]) -> Vec<u8> {
    if extra.is_empty() {
        // ALLOC: one small body, once an index created.
        return settings.to_vec();
    }
    // ALLOC: one small body, once an index created.
    let mut out: Vec<u8> = settings
        .strip_suffix(b"}")
        .map_or_else(|| vec![b'{'], <[u8]>::to_vec);
    let mut first = out.len() <= 1;
    for (key, value) in extra {
        if !first {
            out.push(b',');
        }
        first = false;
        let _ = oss_rest::write_json_str(&mut out, key);
        out.push(b':');
        let _ = oss_rest::write_json_str(&mut out, value);
    }
    out.push(b'}');
    out
}
