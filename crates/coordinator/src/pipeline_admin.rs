//! What the ingest pipeline admin routes decide: whether a definition may
//! be stored, which stored ones an id selects, and how a get answers.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/PipelineHandler.java`
//! (`put`, `get`, `concreteId`). The Java validates a pipeline by compiling it
//! with the node's processor engine; so is this: a pipeline is stored only
//! when [`Pipeline::compile`] accepts it, so a pipeline is never stored that
//! the node cannot run.

use oss_doc::mapping::json::{Json, parse, write_str};

use crate::ingest::Pipeline;
use crate::route::resolve::Refusal;

/// The processors a stored pipeline may name.
pub use crate::ingest::PROCESSORS as SUPPORTED;

fn invalid(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "invalid_pipeline", reason)
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

/// Checks a pipeline definition before it is stored.
///
/// # Errors
///
/// 400 `malformed_body` for a body that is not one JSON object; 400
/// `invalid_pipeline` for a pipeline without `processors`, a processor that
/// is not an object of one type, or a type not in [`SUPPORTED`].
pub fn validate(body: &[u8]) -> Result<(), Refusal> {
    let Ok(Json::Obj(_)) = parse(body) else {
        return Err(Refusal::new(
            400,
            "malformed_body",
            "could not parse the pipeline: the body is not a JSON object",
        ));
    };
    Pipeline::compile(body).map(drop).map_err(invalid)
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
