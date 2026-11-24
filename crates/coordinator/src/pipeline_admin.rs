//! What the ingest pipeline admin routes decide: whether a definition may
//! be stored, which stored ones an id selects, and how a get answers.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/PipelineHandler.java`
//! (`put`, `get`, `concreteId`). The Java validates a pipeline by compiling it
//! with the node's processor engine; here a pipeline is stored only when
//! every processor it names, including those under `on_failure`, is one of
//! [`SUPPORTED`], so a pipeline is never stored that the node cannot run.

use oss_doc::mapping::json::{Json, parse, write_str};

use crate::route::resolve::Refusal;

/// The processors a stored pipeline may name.
pub const SUPPORTED: &[&str] = &[
    "append",
    "convert",
    "lowercase",
    "remove",
    "rename",
    "set",
    "trim",
    "uppercase",
];

/// The deepest `on_failure` nesting a pipeline may carry.
const MAX_DEPTH: usize = 8;

fn invalid(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "invalid_pipeline", reason)
}

fn available() -> String {
    // ALLOC: error path; one list.
    SUPPORTED.join(", ")
}

/// An id a mutation may act on: one concrete id.
///
/// # Errors
///
/// 501 `not_implemented` for an underscore-prefixed id (an API this node
/// does not implement), and for a blank id or one with a wildcard or comma.
pub fn concrete_id(id: &str) -> Result<(), Refusal> {
    if id.starts_with('_') || id.trim().is_empty() || id.contains(['*', '?', ',']) {
        return Err(Refusal::new(
            501,
            "not_implemented",
            "pipeline mutations require one concrete id that does not start with an underscore",
        ));
    }
    Ok(())
}

fn processors(list: &Json, depth: usize) -> Result<(), Refusal> {
    let Json::Arr(items) = list else {
        return Err(invalid("[processors] must be an array of processors"));
    };
    for item in items {
        let Json::Obj(members) = item else {
            return Err(invalid("a processor must be an object of one type"));
        };
        let [(kind, config)] = members.as_slice() else {
            return Err(invalid("a processor must be an object of one type"));
        };
        if !SUPPORTED.contains(&kind.as_str()) {
            // ALLOC: error path; one message.
            return Err(invalid(format!(
                "processor type [{kind}] is not supported. Available processors: {}",
                available()
            )));
        }
        if let Some(handlers) = config.get("on_failure") {
            if depth >= MAX_DEPTH {
                return Err(invalid("[on_failure] is nested too deeply"));
            }
            processors(handlers, depth.saturating_add(1))?;
        }
    }
    Ok(())
}

/// Checks a pipeline definition before it is stored.
///
/// # Errors
///
/// 400 `malformed_body` for a body that is not one JSON object; 400
/// `invalid_pipeline` for a pipeline without `processors`, a processor that
/// is not an object of one type, or a type not in [`SUPPORTED`].
pub fn validate(body: &[u8]) -> Result<(), Refusal> {
    let Ok(parsed @ Json::Obj(_)) = parse(body) else {
        return Err(Refusal::new(
            400,
            "malformed_body",
            "could not parse the pipeline: the body is not a JSON object",
        ));
    };
    let Some(list) = parsed.get("processors") else {
        return Err(invalid("[processors] required property is missing"));
    };
    processors(list, 0)?;
    if let Some(handlers) = parsed.get("on_failure") {
        processors(handlers, 1)?;
    }
    Ok(())
}

/// The stored pipelines an id selects: all for `None`, `*` and `_all`, else
/// the one.
///
/// # Errors
///
/// 501 `not_implemented` for several ids (`a,b`), a wildcard other than `*`,
/// or an underscore-prefixed id; 404 `pipeline_missing` for an id nothing
/// holds.
pub fn select<'a>(
    docs: impl Iterator<Item = (&'a str, &'a [u8])>,
    id: Option<&str>,
) -> Result<Vec<(&'a str, &'a [u8])>, Refusal> {
    // ALLOC: the documents, at most the set's cap.
    let all: Vec<(&str, &[u8])> = docs.collect();
    let Some(id) = id.filter(|i| !matches!(*i, "*" | "_all")) else {
        return Ok(all);
    };
    concrete_id(id).map_err(|_| {
        Refusal::new(
            501,
            "not_implemented",
            "pipeline reads currently require one concrete ID or all pipelines",
        )
    })?;
    match all.into_iter().find(|(n, _)| *n == id) {
        // ALLOC: one entry.
        Some(one) => Ok(vec![one]),
        None => Err(Refusal::new(
            404,
            "pipeline_missing",
            // ALLOC: error path; one message.
            format!("no such pipeline: {id}"),
        )),
    }
}

/// The body of a get: an object of each id to its definition.
pub fn render(found: &[(&str, &[u8])]) -> String {
    // ALLOC: the answer, the size of the definitions it holds.
    let mut out = String::from("{");
    for (i, (id, body)) in found.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_str(&mut out, id);
        out.push(':');
        out.push_str(&String::from_utf8_lossy(body));
    }
    out.push('}');
    out
}
