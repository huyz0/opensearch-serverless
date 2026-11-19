//! A JSON settings object as core's flat settings: dotted keys, each carrying
//! the `index.` prefix unless it already names itself.

use std::collections::BTreeMap;

use oss_doc::mapping::json::Json;

/// The prefix core puts on every index setting.
const INDEX_PREFIX: &str = "index.";

/// The prefix of settings core has set aside rather than understood.
const ARCHIVED_PREFIX: &str = "archived.";

/// Flattens `members` into dotted keys with core's `index.` prefix, sorted by
/// key. Nested objects join their keys with `.`; a value is its text (a
/// string as written, a number as written, a boolean as `true` or `false`, an
/// array as compact JSON) or `None` for null; a later key replaces an earlier
/// one that flattens to the same name.
pub fn flatten(members: &[(String, Json)]) -> Vec<(String, Option<String>)> {
    // ALLOC: one entry per leaf of the request body, which the edge bounds.
    let mut flat = BTreeMap::new();
    for (key, value) in members {
        walk(key, value, &mut flat);
    }
    let named: BTreeMap<_, _> = flat
        .into_iter()
        .map(|(key, value)| (prefixed(key), value))
        // ALLOC: the same entries again, under their final names.
        .collect();
    // ALLOC: the same entries as a list.
    named.into_iter().collect()
}

fn walk(key: &str, value: &Json, flat: &mut BTreeMap<String, Option<String>>) {
    let leaf = match value {
        Json::Obj(members) => {
            for (name, inner) in members {
                // ALLOC: one dotted key, bounded by the body's bytes.
                walk(&format!("{key}.{name}"), inner, flat);
            }
            return;
        }
        Json::Null => None,
        // ALLOC: one value's text, bounded by the body's bytes.
        Json::Bool(b) => Some(b.to_string()),
        Json::Num(text) | Json::Str(text) => Some(text.clone()),
        Json::Arr(_) => Some(value.to_json()),
    };
    // ALLOC: one key, bounded by the body's bytes.
    flat.insert(key.to_owned(), leaf);
}

fn prefixed(key: String) -> String {
    if key.starts_with(INDEX_PREFIX) || key.ends_with('*') || key.starts_with(ARCHIVED_PREFIX) {
        key
    } else {
        // ALLOC: one key, bounded by the body's bytes.
        format!("{INDEX_PREFIX}{key}")
    }
}
