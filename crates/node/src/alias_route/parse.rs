//! Reading what an alias request asks for: literal names, the write flag and
//! the list of actions.

use oss_coordinator::indices::admin::check_name;
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_rest::{Exception, RestError};

pub(super) fn refuse(status: u16, kind: &'static str, reason: impl Into<String>) -> RestError {
    RestError::exception(status, Exception::new(kind, reason))
}

pub(super) fn unsupported(reason: impl Into<String>) -> RestError {
    refuse(501, "unsupported_alias_option", reason)
}

/// One change of one alias.
pub(super) struct Change {
    pub(super) add: bool,
    pub(super) index: String,
    pub(super) alias: String,
    pub(super) write: Option<bool>,
}

/// The body keys of an alias that this node refuses by name.
const REFUSED_KEYS: &[&str] = &[
    "filter",
    "routing",
    "index_routing",
    "search_routing",
    "is_hidden",
    "must_exist",
];

pub(super) fn write_flag(object: &[(String, Json)]) -> Result<Option<bool>, RestError> {
    if let Some(key) = REFUSED_KEYS.iter().find(|k| {
        object
            .iter()
            .any(|(name, v)| name == **k && *v != Json::Null)
    }) {
        return Err(unsupported(format!(
            "[{key}] on an alias is not supported by this node"
        )));
    }
    match object.iter().find(|(k, _)| k == "is_write_index") {
        None | Some((_, Json::Null)) => Ok(None),
        Some((_, Json::Bool(b))) => Ok(Some(*b)),
        Some(_) => Err(RestError::illegal_argument(
            "[is_write_index] must be a boolean",
        )),
    }
}

/// The names under `singular` or `plural` of one action, literal each.
pub(super) fn named(
    action: &[(String, Json)],
    singular: &str,
    plural: &str,
) -> Result<Vec<String>, RestError> {
    let mut out = Vec::new();
    for (key, many) in [(singular, false), (plural, true)] {
        match action.iter().find(|(k, _)| k == key).map(|(_, v)| v) {
            None => {}
            Some(Json::Str(s)) => out.push(s.clone()),
            Some(Json::Arr(items)) if many => {
                for item in items {
                    let Json::Str(s) = item else {
                        return Err(RestError::illegal_argument(format!(
                            "[{key}] must hold strings"
                        )));
                    };
                    out.push(s.clone());
                }
            }
            Some(_) => {
                return Err(RestError::illegal_argument(format!("[{key}] is malformed")));
            }
        }
    }
    if out.is_empty() {
        return Err(RestError::illegal_argument(format!(
            "[{singular}] or [{plural}] is required"
        )));
    }
    for name in &out {
        check_literal(name)?;
    }
    Ok(out)
}

pub(super) fn parse_actions(body: &[u8]) -> Result<Vec<Change>, RestError> {
    let bad = |reason: &str| RestError::illegal_argument(reason.to_owned());
    let parsed =
        parse_json(body).map_err(|e| RestError::illegal_argument(format!("invalid body: {e}")))?;
    let Json::Obj(top) = parsed else {
        return Err(bad("the body must be an object"));
    };
    let Some((_, Json::Arr(actions))) = top.iter().find(|(k, _)| k == "actions") else {
        return Err(bad("[actions] is required"));
    };
    let mut changes = Vec::new();
    for action in actions {
        let Json::Obj(members) = action else {
            return Err(bad("an action names one of add or remove"));
        };
        let [(kind, spec)] = members.as_slice() else {
            return Err(bad("an action names one of add or remove"));
        };
        let add = match kind.as_str() {
            "add" => true,
            "remove" => false,
            "remove_index" => return Err(unsupported("[remove_index] is not supported")),
            other => {
                return Err(RestError::illegal_argument(format!(
                    "unknown action [{other}]"
                )));
            }
        };
        let Json::Obj(spec) = spec else {
            return Err(bad("an action's body must be an object"));
        };
        let write = write_flag(spec)?;
        let indices = named(spec, "index", "indices")?;
        let aliases = named(spec, "alias", "aliases")?;
        for index in &indices {
            for alias in &aliases {
                changes.push(Change {
                    add,
                    index: index.clone(),
                    alias: alias.clone(),
                    write,
                });
            }
        }
    }
    Ok(changes)
}

/// A literal name: no pattern, `_all`, comma list or reserved prefix.
pub(super) fn literal(req: &oss_rest::Request<'_>, param: &str) -> Result<String, RestError> {
    let name = req
        .path_param(param)
        .ok_or_else(|| RestError::illegal_argument(format!("a {param} is required")))?;
    check_literal(&name)?;
    Ok(name.into_owned())
}

pub(super) fn check_literal(name: &str) -> Result<(), RestError> {
    if name.contains(['*', ',']) {
        return Err(unsupported(format!(
            "[{name}] is a pattern or a list: aliases are addressed by literal name, a pattern needs a listing"
        )));
    }
    check_name(name, true).map_err(|r| refuse(r.status, r.kind, r.reason))
}
