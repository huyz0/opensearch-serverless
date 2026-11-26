//! `GET|POST /_render/template` and `GET|POST [/{index}]/_search/template`.
//!
//! Ported from `RestRenderSearchTemplateAction`, `RestSearchTemplateAction`,
//! `TransportSearchTemplateAction` and `MustacheScriptEngine`
//! (modules/lang-mustache) at OpenSearch 3.10.0 for an inline template, or a
//! stored one a [`TemplateSource`] finds. The
//! template renders in `oss_reader::template`; the text is parsed as a search
//! body and run through `search_bytes`, as `_msearch` runs its items.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::dsl::source::tokenizer;
use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource};
use oss_reader::template::{TemplateError, TemplateRequest, Val, render};
use oss_rest::{Exception, JSON, Meta, Method, Request, Response, RestError, RouteError, Router};

use super::{Answer, SearchService, bad_request, record_body, send};

/// The URL parameters `_search/template` takes, as `_search` does.
const TEMPLATE_PARAMS: &[&str] = &["size", "from"];

/// The URL parameters `_render/template` takes: none.
const RENDER_PARAMS: &[&str] = &[];

/// The most bytes a template may render.
pub(super) const MAX_RENDERED_BYTES: usize = MAX_RECORDED_BYTES;

/// The routes of both endpoints, by GET and POST.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for path in ["/_search/template", "/{index}/_search/template"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                TEMPLATE_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_search_template(req, resp)
                },
            )?;
        }
        for path in ["/_render/template", "/_render/template/{id}"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                RENDER_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_render_template(req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

/// Why a stored template could not be had.
#[derive(Debug)]
pub enum TemplateFault {
    /// No script is stored under the id.
    Missing,
    /// The script is stored in a language other than mustache.
    WrongLang(String),
    /// The stored set could not be read.
    Unreadable(String),
}

/// Where a request that names a stored template finds its source.
pub trait TemplateSource: Send + Sync {
    /// The bytes a request reserves from the request-body pool before it
    /// asks for a template: what reading the set holds.
    fn reserve_bytes(&self) -> usize;

    /// The mustache source stored under `id`.
    ///
    /// # Errors
    ///
    /// [`TemplateFault`] for an id nothing is stored under, a script of
    /// another language, or a set that cannot be read.
    fn resolve(&self, id: &str) -> Result<String, TemplateFault>;
}

fn not_supported(what: &str) -> RestError {
    // ALLOC: error path; one message.
    RestError::illegal_argument(format!("{what} is not supported by this node yet"))
}

/// The error a template that did not render gets.
pub(super) fn template_error(e: TemplateError, source: &str) -> RestError {
    match e {
        TemplateError::Compile(message) => {
            let kind = if message.starts_with("Partial not found") {
                "mustache_not_found_exception"
            } else {
                "mustache_exception"
            };
            // ALLOC: error path; one exception chain.
            let cause = Exception::new(kind, message.clone());
            RestError::exception(
                400,
                Exception::new("script_exception", message)
                    .with_meta("script_stack", Meta::EmptyList)
                    .with_meta("script", Meta::Str(source.to_owned()))
                    .with_meta("lang", Meta::Str("mustache".to_owned()))
                    .caused_by(cause),
            )
        }
        TemplateError::Illegal(message) => RestError::illegal_argument(message),
        TemplateError::Unsupported(what) => not_supported(&what),
        TemplateError::Runtime(message) => RestError::exception(
            500,
            // ALLOC: error path; one exception.
            Exception::new("general_script_exception", message)
                .with_meta("script_stack", Meta::EmptyList)
                .with_meta("script", Meta::Str(source.to_owned()))
                .with_meta("lang", Meta::Str("mustache".to_owned())),
        ),
        TemplateError::TooLarge(limit) => {
            // ALLOC: error path; one message.
            RestError::illegal_argument(format!(
                "the rendered template passed {limit} bytes, which this node does not support yet"
            ))
        }
    }
}

/// What a template request names, rendered.
struct Rendered {
    text: String,
    explain: bool,
}

/// The template a request names, rendered: where `id` is the path's, and
/// `hold` reserves what reading a stored template holds.
struct Named<'a> {
    id: Option<&'a str>,
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Installs where a stored template is found. False when one is
    /// installed already.
    pub fn set_stored_templates(&self, source: Arc<dyn TemplateSource>) -> bool {
        self.templates.set(source).is_ok()
    }

    /// The mustache source stored under `id`, after `hold` reserved the
    /// bytes reading it takes.
    pub(super) fn stored_source(
        &self,
        id: &str,
        hold: impl FnOnce(usize) -> Result<(), Answer>,
    ) -> Result<String, Answer> {
        let Some(templates) = self.templates.get() else {
            return Err(Answer::Rest(not_supported("a stored template [id]")));
        };
        hold(templates.reserve_bytes())?;
        templates.resolve(id).map_err(|fault| match fault {
            TemplateFault::Missing => Answer::Rest(RestError::exception(
                404,
                // ALLOC: error path; one exception.
                Exception::new(
                    "resource_not_found_exception",
                    format!("unable to find script [{id}] in cluster state"),
                ),
            )),
            TemplateFault::WrongLang(lang) => Answer::Rest(RestError::illegal_argument(format!(
                // ALLOC: error path; one message.
                "script [{id}] is stored in lang [{lang}], and this node runs only [mustache] templates"
            ))),
            TemplateFault::Unreadable(why) => Answer::Rest(RestError::exception(
                503,
                // ALLOC: error path; one exception.
                Exception::new(
                    "unavailable_shards_exception",
                    format!("stored templates could not be read: {why}"),
                ),
            )),
        })
    }

    /// Renders the template request `rec` recorded; `path` names the stored
    /// template of `/_render/template/{id}`, which wins over the body's.
    fn render_recorded(
        &self,
        rec: &Recorder,
        path: Named<'_>,
        hold: impl FnOnce(usize) -> Result<(), Answer>,
    ) -> Result<Rendered, Answer> {
        let request = TemplateRequest::parse(rec).map_err(bad_request)?;
        if request.profile {
            return Err(Answer::Rest(not_supported(
                "a template request with [profile]",
            )));
        }
        if request.search_pipeline {
            return Err(Answer::Rest(not_supported(
                "a template request with a [search_pipeline]",
            )));
        }
        let stored = match (path.id, request.id.as_deref(), request.stored) {
            (Some(id), _, _) => Some(id),
            (None, Some(id), true) => Some(id),
            _ => None,
        };
        let source = if let Some(id) = stored {
            self.stored_source(id, hold)?
        } else if let Some(source) = request.source {
            source
        } else {
            return Err(Answer::Rest(RestError::exception(
                400,
                Exception::new(
                    "action_request_validation_exception",
                    "Validation Failed: 1: template is missing;",
                ),
            )));
        };
        let params = request.params.unwrap_or(Val::Map(Vec::new()));
        let text = render(&source, &params, MAX_RENDERED_BYTES)
            .map_err(|e| Answer::Rest(template_error(e, &source)))?;
        Ok(Rendered {
            text,
            explain: request.explain,
        })
    }

    /// Reads the request and renders its template, reserving the text.
    fn render_request(&self, req: &mut Request<'_>) -> Result<Rendered, Answer> {
        let rec = record_body(req)?;
        let path = req.path_param("id").map(|id| id.into_owned());
        let pool = req.body();
        let rendered = self.render_recorded(
            &rec,
            Named {
                id: path.as_deref(),
            },
            |n| pool.reserve(n).map_err(Answer::Rest),
        )?;
        req.body()
            .reserve(rendered.text.len())
            .map_err(Answer::Rest)?;
        Ok(rendered)
    }
}

/// The search body the rendered text is.
pub(super) fn rendered_source(text: &str, explain: bool) -> Result<SearchSource, DslError> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokens = tokenizer();
    tokens.feed(text.as_bytes(), &mut rec)?;
    tokens.finish(&mut rec)?;
    let mut source = SearchSource::parse(&rec)?.refuse_pipeline()?;
    source.explain |= explain;
    Ok(source)
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// The search an `_msearch/template` item's body line stands for: its
    /// template rendered, the text held in the request-body pool while it is
    /// parsed.
    pub(super) fn template_item(&self, rec: &Recorder) -> Result<SearchSource, Answer> {
        let mut read = None;
        let rendered = self.render_recorded(rec, Named { id: None }, |n| {
            self.budget
                .try_reserve(oss_mem::Pool::RequestBodies, n)
                .map(|g| read = Some(g))
                .map_err(|e| Answer::Rest(RestError::circuit_breaking(&e, "search_template")))
        })?;
        let _held = self
            .budget
            .try_reserve(oss_mem::Pool::RequestBodies, rendered.text.len())
            .map_err(|_| {
                Answer::Rest(RestError::from(std::io::Error::other(
                    "the request-body pool is full",
                )))
            })?;
        rendered_source(&rendered.text, rendered.explain).map_err(bad_request)
    }

    /// Handles one `_render/template`.
    pub fn handle_render_template(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let rendered = match self.render_request(req) {
            Ok(r) => r,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        // ALLOC: one response, bounded by the reserved rendered text.
        let mut out = Vec::with_capacity(rendered.text.len().saturating_add(24));
        out.extend_from_slice(b"{\"template_output\":");
        out.extend_from_slice(rendered.text.as_bytes());
        out.push(b'}');
        resp.send(200, JSON, &out)
    }

    /// Handles one `_search/template`.
    pub fn handle_search_template(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let rendered = match self.render_request(req) {
            Ok(r) => r,
            Err(Answer::Rest(e)) => return Err(e),
            Err(Answer::Search(e)) => return send(resp, &e),
        };
        let Some(index) = req.path_param("index") else {
            return Err(not_supported("_search/template without an index"));
        };
        let from = req.param("from");
        let size = req.param("size");
        let mut source = match rendered_source(&rendered.text, rendered.explain) {
            Ok(s) => s,
            Err(e) => return bad_answer(resp, e),
        };
        if let Err(e) = source.apply_params(from.as_deref(), size.as_deref()) {
            return bad_answer(resp, e);
        }
        match self.search_bytes(&index, &source) {
            Ok(bytes) => resp.send(200, JSON, &bytes),
            Err(Answer::Rest(e)) => Err(e),
            Err(Answer::Search(e)) => send(resp, &e),
        }
    }
}

fn bad_answer(resp: &mut Response<'_>, e: DslError) -> Result<(), RestError> {
    match bad_request(e) {
        Answer::Search(e) => send(resp, &e),
        Answer::Rest(e) => Err(e),
    }
}
