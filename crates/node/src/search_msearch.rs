//! `GET|POST [/{index}]/_msearch`: several searches in one request.
//!
//! Ported from `RestMultiSearchAction` and `MultiSearchResponse`
//! (server/src/main/java/org/opensearch/rest/action/search/RestMultiSearchAction.java,
//! server/src/main/java/org/opensearch/action/search/MultiSearchResponse.java)
//! at OpenSearch 3.10.0 for the `index` header key. The body streams a line
//! at a time; each search runs as its body line ends and its item is written
//! at once, so only one search's buffers are held at a time.

use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use lucene_analysis::Analyzer;
use oss_blob::BlobStore;
use oss_mem::{Pool, Reservation};
use oss_reader::dsl::source::tokenizer;
use oss_reader::dsl::tokens::Kind;
use oss_reader::dsl::{DslError, MAX_RECORDED_BYTES, Recorder, SearchSource};
use oss_reader::failure::{SearchError, ShardFailure};
use oss_reader::response::write_response;
use oss_reader::search::{hits_bytes, search_with};
use oss_rest::json::Tokenizer;
use oss_rest::ndjson::{LineSink, Splitter};
use oss_rest::{
    BodyWriter, JSON, Length, Method, Request, Response, RestError, RouteError, Router,
};

use super::{Answer, SearchService, bad_request};

/// The most bytes a header line may record.
const MAX_HEADER_BYTES: usize = 64 << 10;

/// The routes of `_msearch` and `_msearch/template`, with and without an
/// index, by GET and POST.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for (path, template) in [
            ("/_msearch", false),
            ("/{index}/_msearch", false),
            ("/_msearch/template", true),
            ("/{index}/_msearch/template", true),
        ] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                &[],
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.msearch(req, resp, template)
                },
            )?;
        }
    }
    Ok(router)
}

fn not_supported(what: &str) -> Vec<u8> {
    // ALLOC: error path; one item body.
    rest_item(&RestError::illegal_argument(format!(
        "{what} is not supported by this node yet"
    )))
}

/// An item that failed: the error body, carrying its status.
fn rest_item(e: &RestError) -> Vec<u8> {
    // ALLOC: error path; one item body.
    let mut out = Vec::new();
    // Writing to a Vec cannot fail.
    let _ = e.write_body(&mut out);
    out
}

pub(super) fn answer_item(answer: Answer) -> Vec<u8> {
    match answer {
        Answer::Rest(e) => rest_item(&e),
        Answer::Search(e) => e.to_body(),
    }
}

/// The index a header names, or why this node cannot search it.
fn header_index(
    rec: &Recorder,
    parts: bool,
    default: Option<&str>,
) -> Result<(String, Option<String>), Vec<u8>> {
    let mut named: Option<String> = None;
    let mut routing: Option<String> = None;
    if parts {
        let mut p = rec.parser();
        if p.next_token() != Some(Kind::StartObject) {
            return Err(rest_item(&RestError::illegal_argument(
                "a _msearch header must be a JSON object",
            )));
        }
        while p.next_token() == Some(Kind::FieldName) {
            let key = p.text().to_owned();
            if !matches!(key.as_str(), "index" | "routing" | "preference") {
                return Err(not_supported(&format!("the _msearch header key [{key}]")));
            }
            if p.next_token() != Some(Kind::String) {
                return Err(not_supported(&format!(
                    "an _msearch [{key}] that is not one string"
                )));
            }
            match key.as_str() {
                "index" => named = Some(p.text().to_owned()),
                "routing" => routing = Some(p.text().to_owned()),
                // No shard has a copy to prefer.
                _ => {}
            }
            p.skip_children();
        }
    }
    let index = named
        .or_else(|| default.map(str::to_owned))
        .ok_or_else(|| not_supported("an _msearch item with no index"))?;
    if index.is_empty() || index == "_all" || index.contains('*') {
        return Err(not_supported("a pattern or _all in _msearch"));
    }
    Ok((index, routing))
}

/// Reads lines and runs each search as its body line ends.
struct Items<'s, 'r, 'a, S: BlobStore + 'static> {
    service: &'s SearchService<S>,
    default: Option<String>,
    /// Each body line is a template request, not a search body.
    template: bool,
    resp: Option<&'r mut Response<'a>>,
    out: Option<BodyWriter<'r, 'a>>,
    items: usize,
    in_body: bool,
    parts: bool,
    header: Recorder,
    header_tokens: Tokenizer,
    header_fail: Option<DslError>,
    body: Recorder,
    body_tokens: Tokenizer,
    body_fail: Option<DslError>,
    /// The index to search, or the item that stands for the refusal.
    target: Result<(String, Option<String>), Vec<u8>>,
}

impl<'s, 'r, 'a, S: BlobStore + 'static> Items<'s, 'r, 'a, S> {
    fn new(
        service: &'s SearchService<S>,
        default: Option<String>,
        template: bool,
        resp: &'r mut Response<'a>,
    ) -> Self {
        Self {
            service,
            default,
            template,
            resp: Some(resp),
            out: None,
            items: 0,
            in_body: false,
            parts: false,
            header: Recorder::new(MAX_HEADER_BYTES),
            header_tokens: tokenizer(),
            header_fail: None,
            body: Recorder::new(MAX_RECORDED_BYTES),
            body_tokens: tokenizer(),
            body_fail: None,
            target: Err(Vec::new()),
        }
    }

    /// The bytes the recorders may grow to when `chunk` more is fed.
    fn bytes_after(&self, chunk: usize) -> usize {
        if self.in_body {
            self.body.bytes_after(chunk)
        } else {
            self.header.bytes_after(chunk)
        }
    }

    /// Writes one item, opening the response with the first.
    fn emit(&mut self, item: &[u8]) -> Result<(), RestError> {
        if self.out.is_none() {
            let Some(resp) = self.resp.take() else {
                return Ok(());
            };
            // The head goes out before the later sub-searches run: a cost
            // header would be partial.
            resp.without_cost_header();
            let mut out = resp.start(200, Some(JSON), Length::Chunked, &[])?;
            out.write_all(b"{\"responses\":[")?;
            self.out = Some(out);
        }
        if let Some(out) = self.out.as_mut() {
            if self.items > 0 {
                out.write_all(b",")?;
            }
            out.write_all(item)?;
        }
        self.items = self.items.saturating_add(1);
        Ok(())
    }

    /// Ends the response with its `took`; `Err` when nothing was answered.
    fn finish(mut self, started: Instant) -> Result<(), RestError> {
        let Some(mut out) = self.out.take() else {
            return Err(RestError::exception(
                400,
                oss_rest::Exception::new(
                    "action_request_validation_exception",
                    "Validation Failed: 1: no requests added;",
                ),
            ));
        };
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        write!(out, "],\"took\":{took}}}")?;
        out.flush()?;
        out.finish()
    }

    /// The search the body line is: parsed, or rendered from its template.
    fn item_source(&self) -> Result<SearchSource, Answer> {
        if self.template {
            self.service.template_item(&self.body)
        } else {
            SearchSource::parse(&self.body).map_err(bad_request)
        }
    }

    fn run(&self, targets: &[String], routing: Option<&str>, source: &mut SearchSource) -> Vec<u8> {
        let [index] = targets else {
            // No index's default pipeline runs over several indexes.
            return Self::answered(self.service.search_bytes_many(targets, source, routing));
        };
        // Held until the item is answered.
        let _pipeline = match self.service.run_item_pipeline(index, source) {
            Ok(held) => held,
            Err(answer) => return answer_item(answer),
        };
        Self::answered(self.service.search_bytes_routed(index, source, routing))
    }

    fn answered(result: Result<Vec<u8>, Answer>) -> Vec<u8> {
        match result {
            // The response ends with `}`; the item's status goes before it.
            Ok(mut bytes) => {
                bytes.pop();
                bytes.extend_from_slice(b",\"status\":200}");
                bytes
            }
            Err(answer) => answer_item(answer),
        }
    }
}

impl<S: BlobStore + 'static> LineSink for Items<'_, '_, '_, S> {
    type Error = RestError;

    fn part(&mut self, bytes: &[u8]) -> Result<(), RestError> {
        self.parts = true;
        if self.in_body {
            if self.body_fail.is_none()
                && let Err(e) = self.body_tokens.feed(bytes, &mut self.body)
            {
                self.body_fail = Some(e);
            }
        } else if self.header_fail.is_none()
            && let Err(e) = self.header_tokens.feed(bytes, &mut self.header)
        {
            self.header_fail = Some(e);
        }
        Ok(())
    }

    fn end_line(&mut self, _: u64) -> Result<(), RestError> {
        let parts = std::mem::take(&mut self.parts);
        if !self.in_body {
            if self.header_fail.is_none()
                && let Err(e) = self.header_tokens.finish(&mut self.header)
            {
                self.header_fail = Some(e);
            }
            self.target = match self.header_fail.take() {
                Some(e) => Err(answer_item(bad_request(e))),
                None => header_index(&self.header, parts, self.default.as_deref()),
            };
            self.header = Recorder::new(MAX_HEADER_BYTES);
            self.header_tokens = tokenizer();
            self.in_body = true;
            return Ok(());
        }
        if self.body_fail.is_none()
            && let Err(e) = self.body_tokens.finish(&mut self.body)
        {
            self.body_fail = Some(e);
        }
        let item = match (
            std::mem::replace(&mut self.target, Err(Vec::new())),
            self.body_fail.take(),
        ) {
            (Err(item), _) => item,
            (Ok(_), Some(e)) => answer_item(bad_request(e)),
            (Ok((index, routing)), None) => match self.service.expand_names(index.split(',')) {
                Err(e) => rest_item(&e),
                Ok(targets) => match self.item_source() {
                    Ok(mut source) => self.run(&targets, routing.as_deref(), &mut source),
                    Err(answer) => answer_item(answer),
                },
            },
        };
        self.body = Recorder::new(MAX_RECORDED_BYTES);
        self.body_tokens = tokenizer();
        self.in_body = false;
        self.emit(&item)
    }
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_msearch`.
    pub fn handle_msearch(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.msearch(req, resp, false)
    }

    /// Handles one `_msearch/template`.
    pub fn handle_msearch_template(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.msearch(req, resp, true)
    }

    fn msearch(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
        template: bool,
    ) -> Result<(), RestError> {
        let started = Instant::now();
        let default = req.path_param("index").map(std::borrow::Cow::into_owned);
        let mut items = Items::new(self, default, template, resp);
        let mut splitter = Splitter::new();
        let body = req.body();
        loop {
            let left = body
                .declared_length()
                .map(|n| n.saturating_sub(body.bytes_read()));
            let chunk = left.map_or(body.max_chunk(), |n| {
                usize::try_from(n).map_or(body.max_chunk(), |n| n.min(body.max_chunk()))
            });
            let more = items.bytes_after(chunk).saturating_sub(body.reserved());
            if more > 0 {
                body.reserve(more)?;
            }
            let Some(chunk) = body.next_chunk()? else {
                break;
            };
            splitter.feed(chunk, &mut items)?;
        }
        if items.parts || items.in_body {
            // A header without its body line, or a line without its newline.
            let reason = if items.parts {
                "The msearch request must be terminated by a newline [\\n]"
            } else {
                "The msearch request is missing the body line of its last header"
            };
            let e = RestError::illegal_argument(reason);
            if items.items == 0 {
                return Err(e);
            }
            items.emit(&rest_item(&e))?;
        }
        items.finish(started)
    }

    /// The response bytes of searching `index` for `source`.
    pub(super) fn search_bytes(
        &self,
        index: &str,
        source: &SearchSource,
    ) -> Result<Vec<u8>, Answer> {
        self.search_bytes_routed(index, source, None)
    }

    /// As [`Self::search_bytes`], over the shards `routing` names.
    pub(super) fn search_bytes_routed(
        &self,
        index: &str,
        source: &SearchSource,
        routing: Option<&str>,
    ) -> Result<Vec<u8>, Answer> {
        let started = Instant::now();
        #[cfg(feature = "coordinator")]
        {
            let (shards, only) = self.shard_plan(index, routing)?;
            if shards > 1 || only.is_some() {
                let visit = super::coordinate::Visit {
                    shards,
                    only: only.as_deref(),
                    members: None,
                };
                return self.coordinate_bytes(index, visit, source, started);
            }
        }
        #[cfg(not(feature = "coordinator"))]
        let _ = routing;
        let held = self.shard(index)?;
        let (held, view) = self.fresh_held(held, true)?;
        let shard = view.as_deref().unwrap_or(&held.shard);
        let mut grants: Vec<Reservation> = Vec::new();
        let mut reserve = |n: usize| {
            self.budget
                .try_reserve(Pool::RequestBodies, n)
                // ALLOC: error path; one box.
                .map_err(|_| std::io::Error::other("the request-body pool is full"))
                // ALLOC: one entry a render.
                .map(|g| grants.push(g))
        };
        reserve(hits_bytes(source, shard)).map_err(|e| Answer::Rest(RestError::from(e)))?;
        let analyzer = Analyzer::standard(None);
        let mut hits = search_with(shard, index, source, &analyzer, held.pinned.as_deref())
            .map_err(|e| {
                let shard_failure = ShardFailure {
                    // ALLOC: error path; the index name.
                    index: index.to_owned(),
                    shard: 0,
                    // ALLOC: error path; the node id.
                    node: self.node_id.clone(),
                };
                Answer::Search(Box::new(SearchError::on_shard(
                    e.status,
                    e.failure,
                    shard_failure,
                )))
            })?;
        if let Some(cap) = source.response_size_cap {
            hits.truncate(cap);
        }
        let took = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        // ALLOC: one response, bounded by the reservations above.
        let mut out = Vec::new();
        write_response(
            &mut out,
            shard,
            index,
            &self.node_id,
            &hits,
            took,
            &mut reserve,
        )
        .map_err(|e| Answer::Rest(RestError::from(e)))?;
        Ok(out)
    }
}
