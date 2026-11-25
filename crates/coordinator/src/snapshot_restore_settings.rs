//! A restore's `index_settings` and `ignore_index_settings`: the settings
//! each created index drops and then takes on, as core's `RestoreService`
//! applies them.

use std::collections::BTreeMap;

use oss_doc::mapping::json::{Json, parse, write_str};

use crate::indices::flat::flatten;
use crate::route::resolve::Refusal;

/// The settings core never lets a restore change or remove.
const FIXED: [&str; 5] = [
    "index.number_of_shards",
    "index.version.created",
    "index.uuid",
    "index.creation_date",
    "index.history.uuid",
];

const INDEX_PREFIX: &str = "index.";

fn refuse(status: u16, kind: &'static str, reason: String) -> Refusal {
    Refusal {
        status,
        kind,
        reason,
    }
}

fn illegal(reason: String) -> Refusal {
    refuse(400, "illegal_argument_exception", reason)
}

/// What a restore does to the settings of the indices it creates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsChange {
    /// Flat `index.` keys and their new text; `None` removes the setting.
    pub set: Vec<(String, Option<String>)>,
    /// Flat `index.` names to drop first; one ending in `*` drops every
    /// setting that starts with what precedes it.
    pub ignore: Vec<String>,
}

fn named(name: &str) -> String {
    let name = name.trim();
    if name.starts_with(INDEX_PREFIX) {
        name.to_owned() // ALLOC: one name, bounded by the 1 MiB body.
    } else {
        format!("{INDEX_PREFIX}{name}") // ALLOC: one name, bounded by the 1 MiB body.
    }
}

impl SettingsChange {
    /// Reads the two body fields.
    ///
    /// # Errors
    ///
    /// 400 `illegal_argument_exception` for a field of the wrong shape;
    /// 400 `snapshot_restore_exception` for a setting core fixes.
    pub fn parse(set: Option<&Json>, ignore: Option<&Json>) -> Result<Self, Refusal> {
        let set = match set {
            None => Vec::new(), // ALLOC: error path; one message.
            // ALLOC: one entry a given setting, bounded by the 1 MiB body.
            Some(Json::Obj(members)) => flatten(members)
                .into_iter()
                .map(|(k, v)| (k, v.map(|v| v.trim().to_owned()))) // ALLOC: one value each.
                .collect(),
            Some(_) => return Err(illegal("index_settings must be an object".to_owned())), // ALLOC: error path; one message.
        };
        let ignore = match ignore {
            None => Vec::new(), // ALLOC: error path; one message.
            // ALLOC: one name each, bounded by the 1 MiB body.
            Some(Json::Str(list)) => list
                .split(',')
                .filter(|n| !n.trim().is_empty())
                .map(named)
                .collect(), // ALLOC: error path; one message.
            Some(Json::Arr(items)) => items
                .iter()
                .map(|item| match item {
                    Json::Str(name) => Ok(named(name)),
                    _ => Err(illegal(
                        "ignore_index_settings must be names of settings".to_owned(), // ALLOC: error path; one message.
                    )),
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(illegal(
                    "ignore_index_settings must be a string or an array of names".to_owned(), // ALLOC: error path; one message.
                ));
            }
        };
        // ALLOC: error path; one message.
        if let Some(name) = ignore
            .iter()
            .find(|n| n.trim_end_matches('*').contains('*'))
        {
            return Err(illegal(format!(
                "ignore_index_settings [{name}]: only a trailing * is supported"
            )));
        }
        for (key, value) in &set {
            if FIXED.contains(&key.as_str()) {
                let verb = if value.is_some() { "modify" } else { "remove" };
                return Err(fixed(verb, key));
            }
        }
        if let Some(name) = ignore.iter().find(|n| FIXED.contains(&n.as_str())) {
            return Err(fixed("remove", name));
        }
        Ok(Self { set, ignore })
    }

    /// Whether the request changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.ignore.is_empty()
    }

    /// The settings object `captured` becomes: flat keys to text, sorted.
    ///
    /// # Errors
    ///
    /// 501 `unsupported_restore` when `captured` is not a flat object of
    /// text, which a change does not handle.
    pub fn merge(&self, captured: Option<&[u8]>) -> Result<Vec<u8>, Refusal> {
        let mut map: BTreeMap<String, String> = BTreeMap::new(); // ALLOC: one entry a setting, bounded by the record.
        if let Some(bytes) = captured {
            let Ok(Json::Obj(members)) = parse(bytes) else {
                return Err(not_flat());
            };
            for (key, value) in members {
                let Json::Str(value) = value else {
                    return Err(not_flat());
                };
                map.insert(named(&key), value);
            }
        }
        for name in &self.ignore {
            match name.strip_suffix('*') {
                Some(prefix) => {
                    map.retain(|k, _| !k.starts_with(prefix) || FIXED.contains(&k.as_str()))
                }
                None => {
                    map.remove(name);
                }
            }
        }
        for (key, value) in &self.set {
            match value {
                Some(value) => {
                    map.insert(key.clone(), value.clone()); // ALLOC: one entry a given setting.
                }
                None => {
                    map.remove(key);
                }
            }
        }
        // ALLOC: the merged object, bounded by the settings read.
        let mut out = String::from("{");
        for (i, (key, value)) in map.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_str(&mut out, key);
            out.push(':');
            write_str(&mut out, value);
        }
        out.push('}');
        Ok(out.into_bytes())
    }
}

fn fixed(verb: &str, key: &str) -> Refusal {
    // ALLOC: error path; one message.
    refuse(
        400,
        "snapshot_restore_exception",
        format!("cannot {verb} setting [{key}] on restore"), // ALLOC: error path; one message.
    )
}

fn not_flat() -> Refusal {
    refuse(
        501,
        "unsupported_restore",
        // ALLOC: error path; one message.
        "the snapshot's index settings are not flat text, so they cannot be changed on restore"
            .to_owned(),
    )
}
