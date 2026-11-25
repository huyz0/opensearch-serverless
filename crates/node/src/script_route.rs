//! `_scripts/{id}`: store, read and remove stored scripts over the document
//! set.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/rest/StoredScriptHandler.java`
//! (its put, get and delete routes), over `oss-meta`'s [`DocSet`] under
//! `scripts/`, the Java's container. A script is stored without being
//! compiled: this node runs none.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_doc::mapping::json::{Json, parse as parse_json};
use oss_format::DocKind;
use oss_meta::{DocSet, MetaError};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router, write_json_str};

use crate::route_util::{read_body, refuse};

/// The query parameters a script request accepts and has nothing to do for.
const PARAMS: &[&str] = &["master_timeout", "cluster_manager_timeout", "timeout"];

/// The most bytes of a script body, core's `script.max_size_in_bytes`.
const MAX_SCRIPT_BYTES: usize = 65_535;

/// The languages core's modules register.
const LANGS: &[&str] = &["painless", "mustache", "expression"];

const ACKNOWLEDGED: &str = "{\"acknowledged\":true}";

/// A script as `{"lang","source"[,"options"]}`.
struct Script {
    lang: String,
    source: String,
    content_type: Option<String>,
}

fn bad(reason: impl Into<String>) -> RestError {
    refuse(400, "illegal_argument_exception", reason)
}

impl Script {
    /// Reads `{"script":{..}}`, as a put or a stored body.
    fn read(body: &[u8]) -> Result<Self, RestError> {
        let parsed = parse_json(body).map_err(|e| bad(format!("invalid script body: {e}")))?;
        let Json::Obj(top) = parsed else {
            return Err(bad("the body must be an object"));
        };
        let [(key, Json::Obj(members))] = top.as_slice() else {
            return Err(bad("the body must hold only a [script] object"));
        };
        if key != "script" {
            return Err(bad("the body must hold only a [script] object"));
        }
        let (mut lang, mut source, mut content_type) = (None, None, None);
        for (name, value) in members {
            match (name.as_str(), value) {
                ("lang", Json::Str(s)) => lang = Some(s.clone()),
                ("source", Json::Str(s)) => source = Some(s.clone()),
                ("options", Json::Obj(options)) => {
                    for (option, value) in options {
                        match (option.as_str(), value) {
                            ("content_type", Json::Str(s)) => content_type = Some(s.clone()),
                            _ => {
                                return Err(bad(format!(
                                    "illegal compiler options [{option}] specified"
                                )));
                            }
                        }
                    }
                }
                _ => {
                    return Err(bad(format!(
                        "unexpected or malformed script field [{name}]"
                    )));
                }
            }
        }
        let lang = lang.ok_or_else(|| bad("script [lang] is required"))?;
        let source = source
            .filter(|s| !s.is_empty())
            .ok_or_else(|| bad("script [source] is required and must not be empty"))?;
        Ok(Self {
            lang,
            source,
            content_type,
        })
    }

    /// The `{"lang","source"[,"options"]}` object.
    fn render(&self) -> String {
        let mut out = String::from("{\"lang\":");
        push_quoted(&mut out, &self.lang);
        out.push_str(",\"source\":");
        push_quoted(&mut out, &self.source);
        if let Some(content_type) = &self.content_type {
            out.push_str(",\"options\":{\"content_type\":");
            push_quoted(&mut out, content_type);
            out.push('}');
        }
        out.push('}');
        out
    }
}

fn push_quoted(out: &mut String, s: &str) {
    let mut bytes = Vec::with_capacity(s.len().saturating_add(2));
    // Writing to a Vec cannot fail.
    let _ = write_json_str(&mut bytes, s);
    out.push_str(&String::from_utf8_lossy(&bytes));
}

/// Serves the stored script routes over the script set.
pub struct ScriptService {
    docs: DocSet,
    buffer_bytes: usize,
}

impl ScriptService {
    /// A service over the set under `base` in `store`; each request holds up
    /// to two buffers of `buffer_bytes`, charged to the request-body pool.
    ///
    /// # Errors
    ///
    /// [`MetaError`] for a base that is not empty and does not end in `/`.
    pub fn new(
        store: &Arc<dyn BlobStore>,
        base: &str,
        buffer_bytes: usize,
    ) -> Result<Self, MetaError> {
        Ok(Self {
            docs: DocSet::new(Arc::clone(store), base, DocKind::StoredScript)?,
            buffer_bytes,
        })
    }

    fn handle(&self, req: &mut Request<'_>, resp: &mut Response<'_>) -> Result<(), RestError> {
        let id = req
            .path_param("id")
            .map(|n| n.into_owned())
            .ok_or_else(|| RestError::illegal_argument("an id is required"))?;
        if id.starts_with('_') {
            return Err(refuse(
                501,
                "not_implemented",
                // ALLOC: error path; one message.
                format!(
                    "'{id}' is not a script id: names beginning with an underscore are reserved for APIs"
                ),
            ));
        }
        match req.method() {
            Method::Put | Method::Post => {
                let body = read_body(
                    req,
                    MAX_SCRIPT_BYTES,
                    "exceeded max allowed stored script size in bytes [65535]",
                )
                .map_err(|_| bad("exceeded max allowed stored script size in bytes [65535]"))?;
                let script = Script::read(&body)?;
                if !LANGS.contains(&script.lang.as_str()) {
                    return Err(bad(format!("script_lang not supported [{}]", script.lang)));
                }
                let stored = format!("{{\"script\":{}}}", script.render());
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                self.docs
                    .put(&id, stored.as_bytes(), false, &mut marker)
                    .map_err(|e| failed(&e))?;
                resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
            }
            Method::Delete => {
                req.body().reserve(self.buffer_bytes)?;
                // ALLOC: one marker buffer a request, buffer_bytes, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                if self.docs.delete(&id, &mut marker).map_err(|e| failed(&e))? {
                    resp.send(200, JSON, ACKNOWLEDGED.as_bytes())
                } else {
                    Err(refuse(
                        404,
                        "resource_not_found_exception",
                        // ALLOC: error path; one message.
                        format!("stored script [{id}] does not exist and cannot be deleted"),
                    ))
                }
            }
            _ => {
                req.body().reserve(self.buffer_bytes.saturating_mul(2))?;
                // ALLOC: two scratch buffers a request, buffer_bytes each, reserved above.
                let mut marker = vec![0u8; self.buffer_bytes];
                let mut doc = vec![0u8; self.buffer_bytes];
                let mut quoted_id = String::new();
                push_quoted(&mut quoted_id, &id);
                let Some(n) = self
                    .docs
                    .get(&id, &mut marker, &mut doc)
                    .map_err(|e| failed(&e))?
                else {
                    let body = format!("{{\"_id\":{quoted_id},\"found\":false}}");
                    return resp.send(404, JSON, body.as_bytes());
                };
                let script = Script::read(&doc[..n]).map_err(|_| {
                    refuse(
                        500,
                        "stored_script_failure",
                        "a stored script is unreadable",
                    )
                })?;
                let body = format!(
                    "{{\"_id\":{quoted_id},\"found\":true,\"script\":{}}}",
                    script.render()
                );
                resp.send(200, JSON, body.as_bytes())
            }
        }
    }

    fn refuse_context(_req: &mut Request<'_>, _resp: &mut Response<'_>) -> Result<(), RestError> {
        Err(refuse(
            501,
            "not_implemented",
            "naming a script context is not supported: core compiles the script against it, which this node cannot do",
        ))
    }
}

/// What a failed store call answers.
fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::TooManyDocs => refuse(400, "too_many_scripts", e.to_string()),
        MetaError::InvalidDocName(_) | MetaError::InvalidArgument(_) => bad(e.to_string()),
        // ALLOC: error path; one message.
        other => refuse(500, "stored_script_failure", other.to_string()),
    }
}

/// The stored script routes: `PUT`, `POST`, `GET` and `DELETE` of one id.
///
/// # Errors
///
/// A route registered twice.
pub fn routes(router: Router, service: &Arc<ScriptService>) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Put, Method::Post, Method::Get, Method::Delete] {
        let service = Arc::clone(service);
        router = router.route(
            method,
            "/_scripts/{id}",
            PARAMS,
            move |req: &mut Request<'_>, resp: &mut Response<'_>| service.handle(req, resp),
        )?;
    }
    for method in [Method::Put, Method::Post] {
        router = router.route(
            method,
            "/_scripts/{id}/{context}",
            PARAMS,
            ScriptService::refuse_context,
        )?;
    }
    Ok(router)
}
