//! What the template admin routes decide: whether a definition may be
//! stored, which stored ones a name selects, and how they are answered.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/TemplateHandler.java`
//! (`put`, `get`, `concreteName`, and the response shapes of core's get
//! actions). A definition is stored as it was sent once it is known to be
//! one JSON object this node can honour; the Java stores core's canonical
//! form of it. Template settings are checked for the shapes the node
//! refuses (replicas above zero); the rest of core's setting validation is
//! not applied here.

use std::fmt::Write;

use oss_doc::mapping::json::{Json, parse, write_str};

use crate::indices::flat::flatten;
use crate::route::resolve::Refusal;

const REPLICAS_KEY: &str = "index.number_of_replicas";

/// Which set a definition belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A composable index template.
    Index,
    /// A component template.
    Component,
}

fn bad(kind: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal::new(400, kind, reason)
}

fn unsupported(kind: &'static str, reason: &'static str) -> Refusal {
    Refusal::new(501, kind, reason)
}

/// A name a mutation may act on: one concrete name, as `concreteName`.
///
/// # Errors
///
/// 501 `not_implemented` for an underscore-prefixed name (an API this node
/// does not implement), and for a blank name or one with a wildcard.
pub fn concrete_name(name: &str) -> Result<(), Refusal> {
    if name.starts_with('_') {
        return Err(Refusal::new(
            501,
            "not_implemented",
            // ALLOC: error path; one message.
            format!(
                "'{name}' is not a template: names beginning with an underscore are reserved for \
                 APIs, and this node does not implement this one"
            ),
        ));
    }
    if name.trim().is_empty() || name.contains(['*', '?']) {
        return Err(unsupported(
            "not_implemented",
            "template mutations require one concrete name",
        ));
    }
    Ok(())
}

/// Checks a definition before it is stored.
///
/// # Errors
///
/// 400 `malformed_body` for a body that is not one JSON object or settings
/// that are not an object; 400 `illegal_argument_exception` for an index
/// template without `index_patterns`, a component template with them, or a
/// `data_stream` that is not an object; 501 `unsupported_template` for
/// `template.aliases`, and 501 `unsupported_setting` for replicas above
/// zero.
pub fn validate(kind: Kind, body: &[u8]) -> Result<(), Refusal> {
    let Ok(parsed @ Json::Obj(_)) = parse(body) else {
        return Err(bad(
            "malformed_body",
            "could not parse the template: the body is not a JSON object",
        ));
    };
    let patterns = parsed.get("index_patterns").filter(|p| **p != Json::Null);
    match (kind, patterns) {
        (Kind::Index, None) => {
            return Err(bad(
                "illegal_argument_exception",
                "an index template must name index_patterns; without them it can never match an index",
            ));
        }
        (Kind::Component, Some(_)) => {
            return Err(bad(
                "illegal_argument_exception",
                "a component template must not name index_patterns: it is composed by an index \
                 template, which decides what it applies to",
            ));
        }
        _ => {}
    }
    let template = parsed.get("template");
    if let Some(Json::Obj(aliases)) = template.and_then(|t| t.get("aliases"))
        && !aliases.is_empty()
    {
        return Err(unsupported(
            "unsupported_template",
            "template.aliases is not supported: aliases are not created with an index here. \
             Create the index, then PUT /{index}/_alias/{name}",
        ));
    }
    if parsed
        .get("data_stream")
        .is_some_and(|d| !matches!(d, Json::Obj(_) | Json::Null))
    {
        return Err(bad(
            "illegal_argument_exception",
            "data_stream must be an object",
        ));
    }
    match template.and_then(|t| t.get("settings")) {
        None | Some(Json::Null) => Ok(()),
        Some(Json::Obj(members)) => {
            let replicas = flatten(members)
                .into_iter()
                .find(|(key, _)| key == REPLICAS_KEY)
                .and_then(|(_, value)| value);
            match replicas.as_deref() {
                None | Some("0") => Ok(()),
                Some(_) => Err(unsupported(
                    "unsupported_setting",
                    "number_of_replicas is not supported: a shard has one writer and its \
                     durability comes from the object store, not from replica copies",
                )),
            }
        }
        Some(_) => Err(bad("malformed_body", "template.settings must be an object")),
    }
}

/// The stored definitions a request name selects: all when `name` is `None`,
/// those starting with the text before a trailing `*`, else the one.
///
/// # Errors
///
/// 501 `not_implemented` for an underscore-prefixed name; 404
/// `template_missing` for a name nothing holds. A wildcard that matches
/// nothing selects nothing.
pub fn select<'a>(
    docs: impl Iterator<Item = (&'a str, &'a [u8])>,
    name: Option<&str>,
) -> Result<Vec<(&'a str, &'a [u8])>, Refusal> {
    // ALLOC: the selected documents, at most the set's cap.
    let all: Vec<(&str, &[u8])> = docs.collect();
    let Some(name) = name else {
        return Ok(all);
    };
    if name.starts_with('_') {
        return Err(unsupported("not_implemented", "reserved template API"));
    }
    if let Some(prefix) = name.strip_suffix('*') {
        return Ok(all
            .into_iter()
            .filter(|(n, _)| n.starts_with(prefix))
            // ALLOC: as above.
            .collect());
    }
    match all.into_iter().find(|(n, _)| *n == name) {
        // ALLOC: one entry.
        Some(one) => Ok(vec![one]),
        None => Err(Refusal::new(
            404,
            "template_missing",
            // ALLOC: error path; one message.
            format!("no such template: {name}"),
        )),
    }
}

/// The body of a get: `{"index_templates":[{"name":..,"index_template":..}]}`
/// or the component form.
pub fn render(kind: Kind, found: &[(&str, &[u8])]) -> String {
    let (list, member) = match kind {
        Kind::Index => ("index_templates", "index_template"),
        Kind::Component => ("component_templates", "component_template"),
    };
    // ALLOC: the answer, the size of the definitions it holds.
    let mut out = String::new();
    let _ = write!(out, "{{\"{list}\":[");
    for (i, (name, body)) in found.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"name\":");
        write_str(&mut out, name);
        let _ = write!(out, ",\"{member}\":");
        out.push_str(&String::from_utf8_lossy(body));
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// What an acknowledged mutation answers.
pub const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";
