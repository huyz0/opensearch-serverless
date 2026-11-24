//! What the pipeline simulate routes decide: how a body names the documents
//! and, inline, the pipeline, and how the run of each document answers.

use oss_doc::mapping::json::{Json, parse, write_str};

use crate::ingest::Pipeline;
use crate::route::resolve::Refusal;

/// The most documents one simulation runs.
pub const MAX_DOCS: usize = 100;

/// One document to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDoc {
    /// `_index`, `_index` when the body gave none.
    pub index: String,
    /// `_id`, `_id` when the body gave none.
    pub id: String,
    /// `_source` as compact JSON.
    pub source: Vec<u8>,
}

/// A simulation request, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simulation {
    /// The inline pipeline definition, none for a stored pipeline.
    pub pipeline: Option<Vec<u8>>,
    /// The documents, in order.
    pub docs: Vec<SimDoc>,
}

fn bad(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "illegal_argument_exception", reason)
}

fn text_of(doc: &Json, key: &str, default: &str) -> Result<String, Refusal> {
    match doc.get(key) {
        // ALLOC: one id, once a document, at most MAX_DOCS.
        None => Ok(default.to_owned()),
        Some(Json::Str(text)) => Ok(text.clone()),
        // ALLOC: error path; one message.
        Some(_) => Err(bad(format!("[{key}] must be a string"))),
    }
}

/// Reads a simulation body. `stored` is a request by pipeline id, which has
/// no inline `pipeline`.
///
/// # Errors
///
/// 400 `malformed_body` for a body that is not a JSON object; 400
/// `illegal_argument_exception` naming the fault for a missing, empty or
/// over-long `docs`, a document that is not an object or has no object
/// `_source`, a non-string `_index` or `_id`, a `pipeline` on a stored
/// request, or none on an inline one; 400 `invalid_pipeline` for an inline
/// definition the node cannot run.
pub fn read(body: &[u8], stored: bool) -> Result<Simulation, Refusal> {
    let Ok(parsed @ Json::Obj(_)) = parse(body) else {
        return Err(Refusal::new(
            400,
            "malformed_body",
            "could not parse the simulation: the body is not a JSON object",
        ));
    };
    let pipeline = match (parsed.get("pipeline"), stored) {
        (Some(_), true) => {
            return Err(bad(
                "[pipeline] is not allowed when the pipeline is named in the path",
            ));
        }
        (None, false) => return Err(bad("[pipeline] required property is missing")),
        (Some(definition), false) => {
            let text = definition.to_json().into_bytes();
            crate::pipeline_admin::validate(&text)?;
            Some(text)
        }
        (None, true) => None,
    };
    let Some(Json::Arr(docs)) = parsed.get("docs") else {
        return Err(bad("[docs] required property is missing"));
    };
    if docs.is_empty() {
        return Err(bad("must specify at least one document in [docs]"));
    }
    if docs.len() > MAX_DOCS {
        // ALLOC: error path; one message.
        return Err(bad(format!(
            "too many documents in [docs]: at most {MAX_DOCS}"
        )));
    }
    // ALLOC: one entry a document, at most MAX_DOCS.
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        let Json::Obj(_) = doc else {
            return Err(bad("each document in [docs] must be an object"));
        };
        let Some(source @ Json::Obj(_)) = doc.get("_source") else {
            return Err(bad("[_source] required property is missing"));
        };
        out.push(SimDoc {
            index: text_of(doc, "_index", "_index")?,
            id: text_of(doc, "_id", "_id")?,
            source: source.to_json().into_bytes(),
        });
    }
    Ok(Simulation {
        pipeline,
        docs: out,
    })
}

/// The answer of running `pipeline` over each of `docs`.
#[must_use]
pub fn render(pipeline: &Pipeline, docs: &[SimDoc]) -> String {
    // ALLOC: one answer, bounded by MAX_DOCS documents of the body's size.
    let mut out = String::from("{\"docs\":[");
    for (at, doc) in docs.iter().enumerate() {
        if at > 0 {
            out.push(',');
        }
        match pipeline.run(&doc.source) {
            Ok(made) => {
                out.push_str("{\"doc\":{\"_index\":");
                write_str(&mut out, &doc.index);
                out.push_str(",\"_id\":");
                write_str(&mut out, &doc.id);
                out.push_str(",\"_source\":");
                out.push_str(&String::from_utf8_lossy(&made));
                out.push_str("}}");
            }
            Err(reason) => {
                // ALLOC: one reason, only for a document that failed.
                let mut quoted = String::new();
                write_str(&mut quoted, &reason);
                // ALLOC: one error object, only for a document that failed.
                out.push_str(&format!(
                    "{{\"error\":{{\"root_cause\":[{{\"type\":\"illegal_argument_exception\",\"reason\":{quoted}}}],\"type\":\"illegal_argument_exception\",\"reason\":{quoted}}}}}"
                ));
            }
        }
    }
    out.push_str("]}");
    out
}
