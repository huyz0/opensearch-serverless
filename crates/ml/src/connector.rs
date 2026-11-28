//! A connector definition's checks, as ml-commons applies them when one is
//! created or changed, and its credential sealed for storage.

use regex::Regex;
use serde_json::{Map, Value};

use crate::credentials;

/// The protocols a connector may name.
const PROTOCOLS: [&str; 5] = [
    "aws_sigv4",
    "http",
    "google_cloud",
    "mcp_sse",
    "mcp_streamable_http",
];

/// The endpoints a connector may call: a URL must match the whole pattern.
/// With no pattern nothing is trusted.
#[derive(Debug, Clone, Default)]
pub struct Trusted(Option<Regex>);

impl Trusted {
    /// A set trusting what `pattern` matches in full; an empty pattern trusts nothing.
    ///
    /// # Errors
    /// The pattern is not a valid regular expression.
    pub fn parse(pattern: &str) -> Result<Self, String> {
        if pattern.trim().is_empty() {
            return Ok(Self(None));
        }
        Regex::new(&format!(r"\A(?:{pattern})\z"))
            .map(|r| Self(Some(r)))
            .map_err(|e| format!("trusted connector endpoints: {e}"))
    }

    /// Whether `url` is one a connector may call.
    #[must_use]
    pub fn allows(&self, url: &str) -> bool {
        self.0.as_ref().is_some_and(|r| r.is_match(url))
    }
}

/// `${parameters.name}` in `text` replaced by the connector's parameter of that
/// name; one with no such parameter stays as written.
fn with_parameters(text: &str, parameters: Option<&Map<String, Value>>) -> String {
    const OPEN: &str = "${parameters.";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once(OPEN) {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            out.push_str(OPEN);
            out.push_str(after);
            return out;
        };
        match parameters.and_then(|p| p.get(name)).and_then(Value::as_str) {
            Some(v) => out.push_str(v),
            None => {
                out.push_str(OPEN);
                out.push_str(name);
                out.push('}');
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

fn text<'a>(def: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    def.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Checks a connector definition; the error is the text stock answers with.
///
/// # Errors
/// The name or protocol is missing or unsupported, an action is malformed,
/// or an action's URL is outside `trusted`.
pub fn validate(def: &Map<String, Value>, trusted: &Trusted) -> Result<(), String> {
    if text(def, "name").is_none() {
        return Err("Connector name is null".into());
    }
    let protocol = text(def, "protocol").ok_or("Connector protocol is null")?;
    if !PROTOCOLS.contains(&protocol) {
        return Err(format!(
            "Unsupported connector protocol. Please use one of [{}]",
            PROTOCOLS.join(", ")
        ));
    }
    validate_actions(def, trusted)
}

fn validate_actions(def: &Map<String, Value>, trusted: &Trusted) -> Result<(), String> {
    let parameters = def.get("parameters").and_then(Value::as_object);
    let actions = match def.get("actions") {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err("Connector actions must be an array".into()),
    };
    for action in actions {
        let action = action
            .as_object()
            .ok_or("Connector actions must be objects")?;
        if text(action, "action_type").is_none() {
            return Err("Connector action type is null".into());
        }
        let url = text(action, "url").ok_or("Connector action url is null")?;
        if !trusted.allows(&with_parameters(url, parameters)) {
            return Err(
                "Connector URL is not matching the trusted connector endpoint regex".into(),
            );
        }
    }
    Ok(())
}

/// The definition as stored: action types upper-cased, every credential value
/// sealed under `master_key`.
///
/// # Errors
/// A credential value is not a string, or sealing fails (the value is never in the text).
pub fn seal_definition(def: &mut Map<String, Value>, master_key: &str) -> Result<(), String> {
    if let Some(Value::Array(actions)) = def.get_mut("actions") {
        for action in actions.iter_mut().filter_map(Value::as_object_mut) {
            if let Some(Value::String(t)) = action.get_mut("action_type") {
                *t = t.to_uppercase();
            }
        }
    }
    match def.get_mut("credential") {
        None | Some(Value::Null) => {}
        Some(Value::Object(creds)) => {
            for value in creds.values_mut() {
                let plain = value
                    .as_str()
                    .ok_or("Connector credential values must be strings")?;
                *value = Value::String(credentials::seal(master_key, plain)?);
            }
        }
        Some(_) => return Err("Connector credential must be an object".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
