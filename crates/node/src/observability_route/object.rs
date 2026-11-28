//! What an observability object body must hold: exactly one of the plugin's
//! object types, as an object with a name.

use serde_json::Value;

/// The object types the plugin stores, as the key that holds the body.
pub(super) const TYPES: &[&str] = &[
    "savedQuery",
    "savedVisualization",
    "operationalPanel",
    "savedNotebook",
    "timestamp",
    "integration",
    "observabilityDashboard",
];

/// The longest name an object may carry.
const MAX_NAME_BYTES: usize = 256;

/// The type key of `body` when it holds exactly one object of a known type
/// whose `name`, when present, is a string of at most 256 bytes.
///
/// # Errors
///
/// A message for a body that is not that.
pub(super) fn checked(body: &Value) -> Result<&'static str, String> {
    let Some(map) = body.as_object() else {
        return Err("the body must be a JSON object".to_owned());
    };
    let mut found: Option<&'static str> = None;
    for key in map.keys() {
        let Some(ty) = TYPES.iter().find(|t| **t == key.as_str()) else {
            return Err(format!(
                "invalid object type [{key}], expected one of {TYPES:?}"
            ));
        };
        if found.replace(ty).is_some() {
            return Err("the body holds more than one object".to_owned());
        }
    }
    let ty = found.ok_or_else(|| "the body holds no object".to_owned())?;
    let object = &map[ty];
    let Some(fields) = object.as_object() else {
        return Err(format!("[{ty}] must be an object"));
    };
    match fields.get("name") {
        None => Ok(ty),
        Some(Value::String(n)) if n.len() <= MAX_NAME_BYTES => Ok(ty),
        Some(Value::String(_)) => Err(format!("[{ty}.name] is over {MAX_NAME_BYTES} bytes")),
        Some(_) => Err(format!("[{ty}.name] must be a string")),
    }
}
