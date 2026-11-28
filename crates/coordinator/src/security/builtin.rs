//! The documents the stock plugin ships: its static roles, action groups and
//! `global_tenant` (built into the plugin, never changed), and the reserved
//! roles of its default configuration (`kibana_user`, `ml_full_access`, ...).
//!
//! `builtin.json` is the recorded 3.10.0 list answers with the stored
//! documents taken out (`fixtures/compat/security-api/security-api.json`).
//! The default configuration's demo users, their mappings and demo tenants
//! are not shipped: they carry well-known credentials.

use std::sync::OnceLock;

use oss_doc::mapping::json::{Json, parse, write_str};

use super::render::rendered_built_in;
use super::{Answer, Kind};

pub(super) const DATA: &str = include_str!("builtin.json");

/// One shipped document.
#[derive(Debug)]
pub struct Builtin {
    /// Its name.
    pub name: String,
    /// `"<name>":{...}`, as it appears in a list answer.
    pub fragment: String,
    /// Whether the plugin builds it in (otherwise it is reserved only).
    pub is_static: bool,
}

type Sets = [Vec<Builtin>; 5];

fn sets() -> &'static Sets {
    static SETS: OnceLock<Sets> = OnceLock::new();
    SETS.get_or_init(|| {
        let doc = parse(DATA.as_bytes()).unwrap_or(Json::Null);
        let build = |kind: Kind, key: &str| -> Vec<Builtin> {
            let Some(Json::Obj(members)) = doc.get(key) else {
                // ALLOC: built once per process.
                return Vec::new();
            };
            members
                .iter()
                .map(|(name, entry)| {
                    let is_static = matches!(entry.get("static"), Some(Json::Bool(true)));
                    let body = entry.get("doc").cloned().unwrap_or(Json::Null);
                    // ALLOC: built once per process.
                    let mut fragment = String::new();
                    write_str(&mut fragment, name);
                    fragment.push(':');
                    fragment.push_str(&rendered_built_in(kind, &body, is_static).to_json());
                    Builtin {
                        name: name.clone(),
                        fragment,
                        is_static,
                    }
                })
                // ALLOC: built once per process.
                .collect()
        };
        [
            build(Kind::Roles, "roles"),
            // ALLOC: built once per process.
            Vec::new(),
            Vec::new(),
            build(Kind::Tenants, "tenants"),
            build(Kind::ActionGroups, "actiongroups"),
        ]
    })
}

fn index(kind: Kind) -> usize {
    Kind::ALL.iter().position(|k| *k == kind).unwrap_or(0)
}

/// The shipped documents of a kind, in name order.
pub fn builtins(kind: Kind) -> &'static [Builtin] {
    sets().get(index(kind)).map_or(&[], Vec::as_slice)
}

/// The shipped document `name`, if there is one.
pub fn builtin(kind: Kind, name: &str) -> Option<&'static Builtin> {
    builtins(kind).iter().find(|b| b.name == name)
}

/// A read answer for a shipped document: `{"<name>": {...}}`.
pub fn read_builtin(kind: Kind, name: &str) -> Option<String> {
    // ALLOC: one answer a request, one shipped document.
    builtin(kind, name).map(|b| format!("{{{}}}", b.fragment))
}

/// The refusal of a change to a shipped document, as the plugin answers it:
/// `static` for one it builds in, `reserved` for the rest.
pub fn protection(kind: Kind, name: &str) -> Option<Answer> {
    let found = builtin(kind, name)?;
    // ALLOC: one message on a refused request.
    let mut text = String::new();
    // ALLOC: one message on a refused request.
    write_str(
        &mut text,
        &format!(
            "Resource '{name}' is {}.",
            if found.is_static {
                "static"
            } else {
                "reserved"
            }
        ),
    );
    Some(Answer {
        status: 403,
        // ALLOC: one message on a refused request.
        body: format!(r#"{{"status":"FORBIDDEN","message":{text}}}"#),
    })
}
