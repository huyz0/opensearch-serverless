//! What a delete-by-query request says: its query and its parameters.

use oss_doc::mapping::json::Json;
use oss_rest::{Request, RestError};

use crate::bulk_route::refresh;
use crate::update_body::{ScriptDef, script};
use oss_types::stored_scripts::StoredScripts;

/// The most bytes of a request body.
const MAX_BODY_BYTES: usize = 1 << 20;

/// The query parameters a delete-by-query accepts. Those with nothing to do
/// here (one node, no throttle) are accepted at their defaults only.
pub(super) const PARAMS: &[&str] = &[
    "conflicts",
    "max_docs",
    "refresh",
    "wait_for_completion",
    "scroll_size",
    "timeout",
    "slices",
    "requests_per_second",
];

/// A parsed request.
pub(super) struct Params {
    /// The `query` object, as JSON text.
    pub query: String,
    /// `max_docs`: the most documents deleted.
    pub max_docs: u64,
    /// `refresh`.
    pub refresh: bool,
    /// `conflicts=abort`, the default.
    pub abort_on_conflict: bool,
    /// An update by query's or a reindex's `script`; `None` for a delete.
    pub script: Option<ScriptDef>,
    /// A reindex's source and destination; `None` for a by-query.
    pub reindex: Option<ReindexTarget>,
    /// `wait_for_completion=false`: run as a task.
    pub background: bool,
}

/// Which request is read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Delete,
    Update,
    Reindex,
}

/// What a reindex copies from and to.
pub(super) struct ReindexTarget {
    /// The source index.
    pub source: String,
    /// The destination index.
    pub dest: String,
    /// `dest.op_type` of `create`: an existing document is a conflict.
    pub create: bool,
}

fn bad(reason: impl Into<String>) -> RestError {
    RestError::illegal_argument(reason)
}

fn unsupported(what: &str) -> RestError {
    // ALLOC: error path; one message.
    bad(format!("{what} is not supported by this node"))
}

impl Params {
    /// Reads the request, refusing what this node does not do.
    ///
    /// `mode`: an update by query's `query` may be absent and its `script`
    /// is required; a reindex names its source and destination.
    pub fn read(
        req: &mut Request<'_>,
        mode: Mode,
        scripts: Option<&dyn StoredScripts>,
    ) -> Result<Self, RestError> {
        let update = mode == Mode::Update;
        let body = read_body(req)?;
        let value = oss_doc::mapping::json::parse(&body).map_err(|e| {
            // ALLOC: error path; one message.
            bad(format!("failed to parse the request body: {}", e.reason))
        })?;
        let conflicts = match (mode, value.get("conflicts")) {
            (Mode::Reindex, Some(Json::Str(c))) => Some(c.clone()), // ALLOC: one small string.
            _ => req.param("conflicts").map(std::borrow::Cow::into_owned),
        };
        let abort_on_conflict = match conflicts.as_deref() {
            None | Some("abort") => true,
            Some("proceed") => false,
            Some(other) => {
                // ALLOC: error path; one message.
                return Err(bad(format!(
                    "conflicts may only be \"proceed\" or \"abort\" but was [{other}]"
                )));
            }
        };
        let background = req
            .param("wait_for_completion")
            .is_some_and(|v| v == "false");
        if req.param("slices").is_some_and(|v| v != "1") {
            return Err(unsupported("slices other than 1"));
        }
        if req.param("requests_per_second").is_some_and(|v| v != "-1") {
            return Err(unsupported("throttling"));
        }
        let in_body = match (mode, value.get("max_docs")) {
            (Mode::Reindex, Some(Json::Num(n))) => Some(n.clone()), // ALLOC: one small number.
            _ => None,
        };
        let max_docs =
            match in_body.or_else(|| req.param("max_docs").map(std::borrow::Cow::into_owned)) {
                None => u64::MAX,
                Some(text) => match text.parse::<i64>() {
                    Ok(n) if n >= 0 => u64::try_from(n).unwrap_or(u64::MAX),
                    _ => {
                        // ALLOC: error path; one message.
                        return Err(bad(format!(
                            "[max_docs] parameter cannot be negative, found [{text}]"
                        )));
                    }
                },
            };
        let refresh = refresh(req.param("refresh").as_deref())?;
        if mode == Mode::Reindex {
            return Self::reindex(
                &value,
                (abort_on_conflict, max_docs, refresh, background),
                scripts,
            );
        }
        let query = match value.get("query") {
            Some(query) => query.to_json(),
            None if update => String::from(r#"{"match_all":{}}"#),
            None => return Err(bad("query is missing")),
        };
        let script = match (update, value.get("script")) {
            (true, Some(def)) => Some(script(def, scripts)?),
            (true, None) => return Err(bad("[script] is required by this node's update by query")),
            (false, _) => None,
        };
        Ok(Self {
            query,
            max_docs,
            refresh,
            abort_on_conflict,
            script,
            reindex: None,
            background,
        })
    }

    /// A reindex body: `source.index`, `source.query`, `dest.index`,
    /// `dest.op_type` and `script`, nothing else.
    fn reindex(
        value: &Json,
        (abort_on_conflict, max_docs, refresh, background): (bool, u64, bool, bool),
        scripts: Option<&dyn StoredScripts>,
    ) -> Result<Self, RestError> {
        let Json::Obj(members) = value else {
            return Err(bad("the request body must be an object"));
        };
        for (key, _) in members {
            if !["source", "dest", "script", "conflicts", "max_docs"].contains(&key.as_str()) {
                // ALLOC: error path; one message.
                return Err(unsupported(&format!("the reindex field [{key}]")));
            }
        }
        let name = |part: &str, key: &str| -> Result<String, RestError> {
            match value.get(part).and_then(|p| p.get(key)) {
                Some(Json::Str(s)) if !s.is_empty() && !s.contains([',', '*']) => Ok(s.clone()), // ALLOC: one name.
                Some(_) => Err(unsupported(&format!(
                    "[{part}.{key}] other than one concrete index"
                ))),
                None => Err(bad(format!("[{part}.{key}] is required"))), // ALLOC: error path.
            }
        };
        let source = name("source", "index")?;
        let dest = name("dest", "index")?;
        for (part, allowed) in [
            ("source", ["index", "query"]),
            ("dest", ["index", "op_type"]),
        ] {
            if let Some(Json::Obj(fields)) = value.get(part)
                && let Some((key, _)) = fields.iter().find(|(k, _)| !allowed.contains(&k.as_str()))
            {
                // ALLOC: error path; one message.
                return Err(unsupported(&format!("[{part}.{key}]")));
            }
        }
        let create = match value.get("dest").and_then(|d| d.get("op_type")) {
            None => false,
            Some(Json::Str(t)) if t == "index" => false,
            Some(Json::Str(t)) if t == "create" => true,
            Some(_) => return Err(bad("[dest.op_type] must be [index] or [create]")),
        };
        let query = value
            .get("source")
            .and_then(|s| s.get("query"))
            .map_or_else(|| String::from(r#"{"match_all":{}}"#), Json::to_json);
        let script = value
            .get("script")
            .map(|def| script(def, scripts))
            .transpose()?;
        Ok(Self {
            query,
            max_docs,
            refresh,
            abort_on_conflict,
            script,
            reindex: Some(ReindexTarget {
                source,
                dest,
                create,
            }),
            background,
        })
    }
}

/// The body, read as it streams in, its growth reserved first.
fn read_body(req: &mut Request<'_>) -> Result<Vec<u8>, RestError> {
    let body = req.body();
    // ALLOC: the body so far, at most MAX_BODY_BYTES, reserved as it grows.
    let mut out: Vec<u8> = Vec::new();
    loop {
        let more = out
            .len()
            .saturating_add(body.max_chunk())
            .saturating_sub(body.reserved());
        body.reserve(more)?;
        let Some(next) = body.next_chunk()? else {
            break;
        };
        if out.len().saturating_add(next.len()) > MAX_BODY_BYTES {
            return Err(bad("the body of a delete by query is larger than 1 MiB"));
        }
        out.extend_from_slice(next);
    }
    if out.is_empty() {
        return Err(bad("request body is required"));
    }
    Ok(out)
}
