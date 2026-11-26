//! `GET|POST /{index}/_rank_eval`: how well a search ranks rated documents.
//!
//! Ported from `RestRankEvalAction`, `TransportRankEvalAction` and the
//! response classes of modules/rank-eval at OpenSearch 3.10.0 for the
//! `precision`, `recall` and `mean_reciprocal_rank` metrics. Each rated
//! request runs as a search of the metric's window size with no `_source`,
//! one at a time through [`SearchService::search_bytes`]; a search that fails
//! is reported under `failures`, any other fault fails the whole request.

use std::io::Write;
use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::dsl::source::tokenizer;
use oss_reader::dsl::tokens::{Kind, Parser};
use oss_reader::dsl::{MAX_RECORDED_BYTES, Recorder, SearchSource};
use oss_reader::fetch::source_filter::SourceFilter;
use oss_reader::rank_eval::metric::{Detail, combine};
use oss_reader::rank_eval::order::{concurrent_map_order, hash_map_order};
use oss_reader::rank_eval::{DocKey, RankEvalSpec, RatedRequest, TemplateText};
use oss_reader::template::{java_double, render};
use oss_rest::{JSON, Method, Request, Response, RestError, RouteError, Router, write_json_str};

use super::msearch::answer_item;
use super::template::{MAX_RENDERED_BYTES, rendered_source, template_error};
use super::{Answer, SearchService, bad_request, record_body, send};

/// The URL parameters `_rank_eval` takes: none yet.
const RANK_EVAL_PARAMS: &[&str] = &[];

/// The routes of `_rank_eval` on one index, by GET and POST.
pub(super) fn routes<S: BlobStore + 'static>(
    router: Router,
    service: Arc<SearchService<S>>,
) -> Result<Router, RouteError> {
    let mut router = router;
    for method in [Method::Get, Method::Post] {
        for path in ["/_rank_eval", "/{index}/_rank_eval"] {
            let service = Arc::clone(&service);
            router = router.route(
                method,
                path,
                RANK_EVAL_PARAMS,
                move |req: &mut Request<'_>, resp: &mut Response<'_>| {
                    service.handle_rank_eval(req, resp)
                },
            )?;
        }
    }
    Ok(router)
}

fn not_supported(what: &str) -> RestError {
    // ALLOC: error path; one message.
    RestError::illegal_argument(format!("{what} is not supported by this node yet"))
}

/// A hit of a rated search.
struct Hit {
    key: DocKey,
    /// `_score` as the search wrote it, `None` for null.
    score: Option<String>,
}

/// The hits of a search response.
fn hits_of(bytes: &[u8]) -> Option<Vec<Hit>> {
    let mut rec = Recorder::new(MAX_RECORDED_BYTES);
    let mut tokens = tokenizer();
    tokens.feed(bytes, &mut rec).ok()?;
    tokens.finish(&mut rec).ok()?;
    let mut p = rec.parser();
    if p.next_token() != Some(Kind::StartObject) {
        return None;
    }
    // ALLOC: one entry a hit, at most the metric's window.
    let mut hits = Vec::new();
    while p.next_token() == Some(Kind::FieldName) {
        if p.text() != "hits" {
            p.next_token();
            p.skip_children();
            continue;
        }
        if p.next_token() != Some(Kind::StartObject) {
            return None;
        }
        while p.next_token() == Some(Kind::FieldName) {
            if p.text() != "hits" {
                p.next_token();
                p.skip_children();
                continue;
            }
            if p.next_token() != Some(Kind::StartArray) {
                return None;
            }
            while p.next_token() == Some(Kind::StartObject) {
                hits.push(hit_of(&mut p)?);
            }
        }
    }
    Some(hits)
}

fn hit_of(p: &mut Parser<'_>) -> Option<Hit> {
    let (mut index, mut id, mut score) = (None, None, None);
    while p.next_token() == Some(Kind::FieldName) {
        let name = p.text();
        let kind = p.next_token();
        match (name, kind) {
            // ALLOC: one string of a hit, bounded by the recorded response.
            ("_index", Some(Kind::String)) => index = Some(p.text().to_owned()),
            // ALLOC: as above.
            ("_id", Some(Kind::String)) => id = Some(p.text().to_owned()),
            // ALLOC: as above.
            ("_score", Some(Kind::Number)) => score = Some(p.text().to_owned()),
            _ => p.skip_children(),
        }
    }
    Some(Hit {
        key: DocKey {
            index: index?,
            id: id?,
        },
        score,
    })
}

/// The search a rated request stands for, as `TransportRankEvalAction` builds it.
///
/// `stored` holds the source of each stored script the request body names.
fn search_source(
    request: &RatedRequest,
    spec: &RankEvalSpec,
    stored: &[(String, String)],
) -> Result<SearchSource, RestError> {
    let text = match (&request.body, &request.template_id) {
        (Some(body), _) => body.clone(),
        (None, Some(id)) => {
            let source = match spec.templates.iter().find(|(t, _)| t == id) {
                Some((_, TemplateText::Inline(source))) => source.as_str(),
                Some((_, TemplateText::Stored(script))) => stored
                    .iter()
                    .find(|(s, _)| s == script)
                    .map(|(_, source)| source.as_str())
                    .unwrap_or_default(),
                None => "",
            };
            render(source, &request.params, MAX_RENDERED_BYTES)
                .map_err(|e| template_error(e, source))?
        }
        (None, None) => String::new(),
    };
    let mut source = rendered_source(&text, false).map_err(|e| match bad_request(e) {
        Answer::Rest(e) => e,
        Answer::Search(e) => RestError::illegal_argument(e.failure.reason.clone()),
    })?;
    for (present, what) in [
        (!source.aggs.is_empty(), "contain aggregations"),
        (source.explain, "use explain"),
        (source.highlight.is_some(), "contain a highlighter section"),
    ] {
        if present {
            return Err(RestError::illegal_argument(if what == "use explain" {
                "Query in rated requests should not use explain.".to_owned()
            } else {
                // ALLOC: error path; one message.
                format!("Query in rated requests should not {what}.")
            }));
        }
    }
    if !source.sort.is_empty() {
        return Err(not_supported("a rated request with a [sort]"));
    }
    source.size = usize::try_from(spec.metric.k()).unwrap_or(0);
    source.source = SourceFilter {
        skip: true,
        ..SourceFilter::default()
    };
    Ok(source)
}

/// `{"error":{..}}` from an item body, which ends with `,"status":N}`.
fn failure_body(item: Vec<u8>) -> Vec<u8> {
    let text = String::from_utf8_lossy(&item);
    match text.rfind(",\"status\":") {
        Some(at) => {
            let mut out = text.get(..at).unwrap_or_default().as_bytes().to_vec();
            out.push(b'}');
            out
        }
        None => item,
    }
}

/// An index named as one concrete name, or why this node cannot search it.
fn single_index(index: &str) -> Result<(), RestError> {
    if index.is_empty() || index == "_all" || index.contains([',', '*']) {
        return Err(not_supported(
            "a pattern, _all or a list of indices in _rank_eval",
        ));
    }
    Ok(())
}

fn write_hit(out: &mut Vec<u8>, hit: &Hit, rating: Option<i32>) -> std::io::Result<()> {
    out.extend_from_slice(b"{\"hit\":{\"_index\":");
    write_json_str(out, &hit.key.index)?;
    out.extend_from_slice(b",\"_id\":");
    write_json_str(out, &hit.key.id)?;
    out.extend_from_slice(b",\"_score\":");
    match &hit.score {
        Some(score) => out.extend_from_slice(score.as_bytes()),
        None => out.extend_from_slice(b"null"),
    }
    out.extend_from_slice(b"},\"rating\":");
    match rating {
        Some(r) => write!(out, "{r}")?,
        None => out.extend_from_slice(b"null"),
    }
    out.push(b'}');
    Ok(())
}

/// One request's entry of `details`, without its key.
fn write_quality(
    out: &mut Vec<u8>,
    hits: &[Hit],
    quality: &oss_reader::rank_eval::Quality,
) -> std::io::Result<()> {
    write!(
        out,
        "{{\"metric_score\":{},\"unrated_docs\":[",
        java_double(quality.score)
    )?;
    let mut first = true;
    for (hit, rating) in hits.iter().zip(&quality.ratings) {
        if rating.is_none() {
            if !first {
                out.push(b',');
            }
            first = false;
            out.extend_from_slice(b"{\"_index\":");
            write_json_str(out, &hit.key.index)?;
            out.extend_from_slice(b",\"_id\":");
            write_json_str(out, &hit.key.id)?;
            out.push(b'}');
        }
    }
    out.extend_from_slice(b"],\"hits\":[");
    for (n, (hit, rating)) in hits.iter().zip(&quality.ratings).enumerate() {
        if n > 0 {
            out.push(b',');
        }
        write_hit(out, hit, *rating)?;
    }
    out.extend_from_slice(b"],\"metric_details\":");
    match quality.detail {
        Detail::Precision {
            relevant_retrieved,
            retrieved,
        } => write!(
            out,
            "{{\"precision\":{{\"relevant_docs_retrieved\":{relevant_retrieved},\"docs_retrieved\":{retrieved}}}}}"
        )?,
        Detail::Recall {
            relevant_retrieved,
            relevant,
        } => write!(
            out,
            "{{\"recall\":{{\"relevant_docs_retrieved\":{relevant_retrieved},\"relevant_docs\":{relevant}}}}}"
        )?,
        Detail::ReciprocalRank { first_relevant } => write!(
            out,
            "{{\"mean_reciprocal_rank\":{{\"first_relevant\":{first_relevant}}}}}"
        )?,
    }
    out.push(b'}');
    Ok(())
}

impl<S: BlobStore + 'static> SearchService<S> {
    /// Handles one `_rank_eval`.
    pub fn handle_rank_eval(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let Some(index) = req.path_param("index").map(std::borrow::Cow::into_owned) else {
            return Err(not_supported("_rank_eval without an index"));
        };
        single_index(&index)?;
        let spec =
            match record_body(req).and_then(|rec| RankEvalSpec::parse(&rec).map_err(bad_request)) {
                Ok(spec) => spec,
                Err(Answer::Rest(e)) => return Err(e),
                Err(Answer::Search(e)) => return send(resp, &e),
            };
        // Each stored script a rated request renders is read once.
        let mut stored: Vec<(String, String)> = Vec::new();
        for request in &spec.requests {
            let Some(id) = &request.template_id else {
                continue;
            };
            let Some((_, TemplateText::Stored(script))) =
                spec.templates.iter().find(|(t, _)| t == id)
            else {
                continue;
            };
            if request.body.is_some() || stored.iter().any(|(s, _)| s == script) {
                continue;
            }
            let pool = req.body();
            match self.stored_source(script, |n| pool.reserve(n).map_err(Answer::Rest)) {
                // ALLOC: one source a stored script the body names, reserved by the read.
                Ok(source) => stored.push((script.clone(), source)),
                Err(Answer::Rest(e)) => return Err(e),
                Err(Answer::Search(e)) => return send(resp, &e),
            }
        }
        // The evaluated requests, in order, and the failed ones.
        let mut done: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut failed: Vec<(&str, Vec<u8>)> = Vec::new();
        let mut scores = Vec::new();
        for request in &spec.requests {
            let source = search_source(request, &spec, &stored)?;
            let bytes = match self.search_bytes(&index, &source) {
                Ok(bytes) => bytes,
                Err(answer) => {
                    failed.push((&request.id, failure_body(answer_item(answer))));
                    continue;
                }
            };
            req.body().reserve(bytes.len())?;
            let Some(hits) = hits_of(&bytes) else {
                return Err(RestError::illegal_argument(
                    "a search response of this node could not be read back",
                ));
            };
            drop(bytes);
            let keys: Vec<DocKey> = hits.iter().map(|h| h.key.clone()).collect();
            let quality = spec.metric.evaluate(&keys, &request.ratings);
            // ALLOC: one entry a request, reserved below.
            let mut entry = Vec::new();
            write_quality(&mut entry, &hits, &quality).map_err(RestError::from)?;
            req.body().reserve(entry.len())?;
            scores.push(quality.score);
            done.push((&request.id, entry));
        }
        // ALLOC: one response, bounded by the reservations above.
        let mut out = Vec::new();
        if scores.is_empty() {
            out.extend_from_slice(b"{\"metric_score\":\"NaN\",\"details\":{");
        } else {
            write!(
                out,
                "{{\"metric_score\":{},\"details\":{{",
                java_double(combine(&scores))
            )
            .map_err(RestError::from)?;
        }
        let ids: Vec<&str> = done.iter().map(|(id, _)| *id).collect();
        for (n, &i) in hash_map_order(&ids, spec.requests.len()).iter().enumerate() {
            let Some((id, entry)) = done.get(i) else {
                continue;
            };
            if n > 0 {
                out.push(b',');
            }
            write_json_str(&mut out, id).map_err(RestError::from)?;
            out.push(b':');
            out.extend_from_slice(entry);
        }
        out.extend_from_slice(b"},\"failures\":{");
        let ids: Vec<&str> = failed.iter().map(|(id, _)| *id).collect();
        for (n, &i) in concurrent_map_order(&ids, spec.requests.len())
            .iter()
            .enumerate()
        {
            let Some((id, body)) = failed.get(i) else {
                continue;
            };
            if n > 0 {
                out.push(b',');
            }
            write_json_str(&mut out, id).map_err(RestError::from)?;
            out.push(b':');
            out.extend_from_slice(body);
        }
        out.extend_from_slice(b"}}");
        resp.send(200, JSON, &out)
    }
}
