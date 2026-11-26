//! `SearchService/Search`: a `SearchRequest` as a REST `_search` call and its answer as a
//! `SearchResponse`. Only the covered subset is read; any other set field is refused by path.

use prost_reflect::{DynamicMessage, Value};
use serde_json::{Map, Value as Json};

use super::fields::Fields;
use super::query::query;
use super::rest::{Query, Rest, decode, join, push_encoded, source_param};
use super::schema;
use super::status::{INVALID_ARGUMENT, Status, UNIMPLEMENTED};

/// Decodes `message` and maps it to the REST request.
pub(crate) fn request(message: &[u8]) -> Result<Rest, Status> {
    let msg = decode(schema::message("SearchRequest"), message)?;
    map(&msg)
}

/// The REST target and the search body object of a decoded `SearchRequest`.
fn plan(msg: &DynamicMessage) -> Result<(String, Map<String, Json>), Status> {
    let mut f = Fields::new(msg, "SearchRequest");
    let mut target = String::from("/");
    let indices: Vec<&str> = f
        .repeated("index")
        .iter()
        .filter_map(Value::as_str)
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        .collect();
    for (i, index) in indices.iter().enumerate() {
        if i > 0 {
            target.push(',');
        }
        push_encoded(&mut target, index);
    }
    if !indices.is_empty() {
        target.push('/');
    }
    target.push_str("_search");
    let mut query = Query::default();
    if let Some(source) = f.message("x_source") {
        source_param(source, "SearchRequest.x_source", &mut query)?;
    }
    query.text(
        "_source_excludes",
        join(f.repeated("x_source_excludes")).as_deref(),
    );
    query.text(
        "_source_includes",
        join(f.repeated("x_source_includes")).as_deref(),
    );
    let mut body = Map::new();
    if let Some(b) = f.message("search_request_body") {
        body_of(b, &f.at("search_request_body"), &mut body)?;
    }
    f.finish()?;
    target.push_str(&query.0);
    Ok((target, body))
}

fn map(msg: &DynamicMessage) -> Result<Rest, Status> {
    let (target, body) = plan(msg)?;
    // ALLOC: the search body, within `grpc_max_message` and charged to the request-body pool at start.
    let body = serde_json::to_vec(&Json::Object(body))
        .map_err(|_| Status::new(INVALID_ARGUMENT, "the search body does not encode"))?;
    Ok(Rest {
        method: "POST",
        target,
        body,
    })
}

/// How many hits a `ServerStreamSearch` message holds when the request names no `size`.
pub(crate) const DEFAULT_PAGE: usize = 100;
/// The most hits a page may hold.
const MAX_PAGE: usize = 10_000;

/// A `ServerStreamSearch` request: the search to page through, and the page size.
pub(crate) struct StreamPlan {
    pub(crate) target: String,
    pub(crate) body: Map<String, Json>,
    pub(crate) page: usize,
}

/// Decodes a `ServerStreamSearch` request. It needs a `sort` to page by, takes `size` as the page
/// size, and refuses `from`, which a stream has no use for.
pub(crate) fn stream_request(message: &[u8]) -> Result<StreamPlan, Status> {
    let msg = decode(schema::message("SearchRequest"), message)?;
    let (target, mut body) = plan(&msg)?;
    if body.remove("from").is_some() {
        return Err(Status::new(
            UNIMPLEMENTED,
            "SearchRequest.search_request_body.from is not supported by ServerStreamSearch",
        ));
    }
    if !body.contains_key("sort") {
        return Err(Status::new(
            INVALID_ARGUMENT,
            "ServerStreamSearch needs search_request_body.sort, the order its pages follow",
        ));
    }
    let page = match body.remove("size").and_then(|n| n.as_u64()) {
        None => DEFAULT_PAGE,
        Some(n @ 1..=10_000) => usize::try_from(n).unwrap_or(MAX_PAGE),
        Some(_) => {
            return Err(Status::new(
                INVALID_ARGUMENT,
                // ALLOC: one page of hits, within `grpc_max_message` and charged to the request-body pool at start.
                format!("ServerStreamSearch page size must be 1 to {MAX_PAGE}"),
            ));
        }
    };
    Ok(StreamPlan { target, body, page })
}

fn body_of(msg: &DynamicMessage, path: &str, out: &mut Map<String, Json>) -> Result<(), Status> {
    let mut f = Fields::new(msg, path);
    if let Some(q) = f.message("query") {
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        out.insert("query".to_owned(), query(q, &f.at("query"))?);
    }
    for name in ["from", "size"] {
        if let Some(n) = f.int(name) {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            out.insert(name.to_owned(), Json::from(n));
        }
    }
    if let Some(t) = f.message("track_total_hits") {
        let mut h = Fields::new(t, &f.at("track_total_hits"));
        if let Some(on) = h.boolean("enabled") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            out.insert("track_total_hits".to_owned(), Json::Bool(on));
        }
        if let Some(n) = h.int("count") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            out.insert("track_total_hits".to_owned(), Json::from(n));
        }
        h.finish()?;
    }
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    let mut sorts = Vec::new();
    for (i, item) in f.repeated("sort").iter().enumerate() {
        let Value::Message(item) = item else { continue };
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        sorts.push(sort(item, &format!("{}[{i}]", f.at("sort")))?);
    }
    if !sorts.is_empty() {
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        out.insert("sort".to_owned(), Json::Array(sorts));
    }
    if let Some(p) = f.message("pit") {
        let mut pit = Fields::new(p, &f.at("pit"));
        let mut o = Map::new();
        if let Some(id) = pit.string("id") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            o.insert("id".to_owned(), Json::from(id));
        }
        if let Some(keep) = pit.string("keep_alive") {
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            o.insert("keep_alive".to_owned(), Json::from(keep));
        }
        pit.finish()?;
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        out.insert("pit".to_owned(), Json::Object(o));
    }
    f.finish()
}

/// One `SortCombinations`: a field name, a field with a direction, or a field with its order.
fn sort(msg: &DynamicMessage, path: &str) -> Result<Json, Status> {
    let mut f = Fields::new(msg, path);
    let mut out = None;
    if let Some(field) = f.string("field") {
        out = Some(Json::from(field));
    }
    if let Some(m) = f.message("field_with_direction") {
        let mut d = Fields::new(m, &f.at("field_with_direction"));
        let mut o = Map::new();
        for (field, v) in d.entries("sort_order_map") {
            o.insert(field, Json::from(order(v)));
        }
        d.finish()?;
        out = Some(Json::Object(o));
    }
    if let Some(m) = f.message("field_with_order") {
        let at = f.at("field_with_order");
        let mut d = Fields::new(m, &at);
        let mut o = Map::new();
        for (field, v) in d.entries("field_sort_map") {
            let Value::Message(sort) = v else { continue };
            // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
            let mut s = Fields::new(sort, &format!("{at}.field_sort_map[{field}]"));
            let mut inner = Map::new();
            if let Some(word) = s.enum_word("order") {
                // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
                inner.insert("order".to_owned(), Json::from(word));
            }
            s.finish()?;
            o.insert(field, Json::Object(inner));
        }
        d.finish()?;
        out = Some(Json::Object(o));
    }
    f.finish()?;
    // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
    out.ok_or_else(|| Status::new(INVALID_ARGUMENT, format!("{path} names no sort")))
}

/// `SORT_ORDER_DESC` as `desc`: the enum number's name, prefix removed.
fn order(value: &Value) -> &'static str {
    match value {
        Value::EnumNumber(2) => "desc",
        _ => "asc",
    }
}

#[cfg(test)]
mod tests {
    use super::super::query::COVERED_QUERIES;
    use super::*;
    use prost::Message;
    use serde_json::json;

    fn encode(json: Json) -> Vec<u8> {
        DynamicMessage::deserialize(schema::message("SearchRequest"), json)
            .expect("protojson")
            .encode_to_vec()
    }

    fn mapped(json: Json) -> (String, Json) {
        let rest = request(&encode(json)).expect("maps");
        assert_eq!(rest.method, "POST");
        let body = serde_json::from_slice(&rest.body).expect("json");
        (rest.target, body)
    }

    fn body(search_body: Json) -> Json {
        mapped(json!({"searchRequestBody": search_body})).1
    }

    #[test]
    fn indices_and_source_parameters_make_the_target() {
        let (target, body) = mapped(
            json!({"index": ["a b", "c"], "xSourceIncludes": ["i"], "xSource": {"fetch": true}}),
        );
        assert_eq!(target, "/a%20b,c/_search?_source=true&_source_includes=i");
        assert_eq!(body, json!({}));
        assert_eq!(mapped(json!({})).0, "/_search");
    }

    #[test]
    fn the_body_carries_paging_sort_track_hits_and_pit() {
        let b = body(json!({
            "from": 3, "size": 7, "trackTotalHits": {"count": 100},
            "sort": [{"field": "a"}, {"fieldWithDirection": {"sortOrderMap": {"b": "SORT_ORDER_DESC"}}},
                     {"fieldWithOrder": {"fieldSortMap": {"c": {"order": "SORT_ORDER_ASC"}}}}],
            "pit": {"id": "p1", "keepAlive": "1m"}
        }));
        assert_eq!(
            b,
            json!({"from": 3, "size": 7, "track_total_hits": 100,
                   "sort": ["a", {"b": "desc"}, {"c": {"order": "asc"}}],
                   "pit": {"id": "p1", "keep_alive": "1m"}})
        );
        assert_eq!(
            body(json!({"trackTotalHits": {"enabled": false}})),
            json!({"track_total_hits": false})
        );
        assert_eq!(
            body(json!({"trackTotalHits": {"enabled": true}})),
            json!({"track_total_hits": true})
        );
    }

    #[test]
    fn each_query_type_has_its_rest_shape() {
        let q = |query: Json| body(json!({"query": query}))["query"].clone();
        let t = |f: &str, v: &str| json!({"term": {"field": f, "value": {"string": v}}});
        assert_eq!(q(json!({"matchAll": {}})), json!({"match_all": {}}));
        assert_eq!(q(t("f", "v")), json!({"term": {"f": "v"}}));
        assert_eq!(
            q(json!({"term": {"field": "f", "value": {"generalNumber": {"int64Value": "5"}}}})),
            json!({"term": {"f": 5}})
        );
        assert_eq!(
            q(json!({"term": {"field": "f", "value": {"generalNumber": {"doubleValue": 2.5}}}})),
            json!({"term": {"f": 2.5}})
        );
        assert_eq!(
            q(json!({"term": {"field": "f", "value": {"generalNumber": {"floatValue": 0.1}}}})),
            json!({"term": {"f": 0.1}})
        );
        assert_eq!(
            q(json!({"term": {"field": "f", "value": {"bool": true}}})),
            json!({"term": {"f": true}})
        );
        assert_eq!(
            q(
                json!({"term": {"field": "f", "value": {"generalNumber": {"uint64Value": "18446744073709551615"}}}})
            ),
            json!({"term": {"f": 18_446_744_073_709_551_615u64}})
        );
        assert_eq!(
            q(json!({"range": {"numberRangeQuery": {"field": "n", "gt": 9e15, "lt": 8e15}}})),
            json!({"range": {"n": {"gt": 9e15, "lt": 8_000_000_000_000_000i64}}})
        );
        assert_eq!(
            q(json!({"term": {"field": "f", "value": {"nullValue": "NULL_VALUE_NULL"}}})),
            json!({"term": {"f": null}})
        );
        assert_eq!(
            q(
                json!({"terms": {"terms": {"f": {"value": {"fieldValueArray": [{"string": "a"}, {"bool": false}]}}}}})
            ),
            json!({"terms": {"f": ["a", false]}})
        );
        assert_eq!(
            q(json!({"match": {"field": "f", "query": {"string": "x y"}}})),
            json!({"match": {"f": "x y"}})
        );
        assert_eq!(
            q(json!({"matchPhrase": {"field": "f", "query": "x y"}})),
            json!({"match_phrase": {"f": "x y"}})
        );
        assert_eq!(
            q(
                json!({"range": {"numberRangeQuery": {"field": "n", "gt": 1.0, "gte": 2.0, "lt": 3.5, "lte": 4.0}}})
            ),
            json!({"range": {"n": {"gt": 1, "gte": 2, "lt": 3.5, "lte": 4}}})
        );
        assert_eq!(
            q(
                json!({"range": {"dateRangeQuery": {"field": "d", "gte": "2020-01-01", "lt": "now"}}})
            ),
            json!({"range": {"d": {"gte": "2020-01-01", "lt": "now"}}})
        );
        assert_eq!(
            q(json!({"exists": {"field": "f"}})),
            json!({"exists": {"field": "f"}})
        );
        assert_eq!(
            q(json!({"ids": {"values": ["1", "2"]}})),
            json!({"ids": {"values": ["1", "2"]}})
        );
        assert_eq!(
            q(json!({"prefix": {"field": "f", "value": "p"}})),
            json!({"prefix": {"f": "p"}})
        );
        assert_eq!(
            q(
                json!({"bool": {"must": [t("a", "1")], "filter": [t("b", "2")], "should": [t("c", "3")],
                              "mustNot": [t("d", "4")], "minimumShouldMatch": {"string": "50%"}}})
            ),
            json!({"bool": {"must": [{"term": {"a": "1"}}], "filter": [{"term": {"b": "2"}}],
                            "should": [{"term": {"c": "3"}}], "must_not": [{"term": {"d": "4"}}],
                            "minimum_should_match": "50%"}})
        );
        assert_eq!(
            q(json!({"bool": {"minimumShouldMatch": {"int32": 2}}})),
            json!({"bool": {"minimum_should_match": 2}})
        );
    }

    #[test]
    fn a_bool_nests_to_any_depth_the_decoder_allows() {
        let mut query = json!({"matchAll": {}});
        for _ in 0..20 {
            query = json!({"bool": {"must": [query]}});
        }
        let mapped = body(json!({"query": query}));
        assert!(mapped["query"]["bool"]["must"][0]["bool"].is_object());
    }

    #[test]
    fn fields_outside_the_subset_are_refused_by_path() {
        for (json, path) in [
            (
                json!({"searchRequestBody": {"aggregations": {"a": {}}}}),
                "SearchRequest.search_request_body.aggregations",
            ),
            (
                json!({"searchRequestBody": {"scriptFields": {"a": {}}}}),
                "SearchRequest.search_request_body.script_fields",
            ),
            (
                json!({"searchRequestBody": {"query": {"knn": {}}}}),
                "SearchRequest.search_request_body.query.knn",
            ),
            (json!({"q": "x"}), "SearchRequest.q"),
            (
                json!({"searchRequestBody": {"query": {"term": {"field": "f", "boost": 2.0, "value": {"string": "v"}}}}}),
                "SearchRequest.search_request_body.query.term.boost",
            ),
            (
                json!({"searchRequestBody": {"query": {"bool": {"must": [{"fuzzy": {}}]}}}}),
                "SearchRequest.search_request_body.query.bool.must[0].fuzzy",
            ),
            (
                json!({"searchRequestBody": {"sort": [{"options": {"xScore": {}}}]}}),
                "SearchRequest.search_request_body.sort[0].options",
            ),
            (
                json!({"searchRequestBody": {"query": {"terms": {"terms": {"f": {"lookup": {}}}}}}}),
                "SearchRequest.search_request_body.query.terms.terms[f].lookup",
            ),
            (
                json!({"searchRequestBody": {"query": {"range": {"dateRangeQuery": {"field": "d", "format": "x"}}}}}),
                "SearchRequest.search_request_body.query.range.date_range_query.format",
            ),
            (
                json!({"searchRequestBody": {"pit": {"id": "p", "keepAlive": "1m"}, "xSource": {"fetch": true}}}),
                "SearchRequest.search_request_body.x_source",
            ),
        ] {
            let refused = request(&encode(json.clone())).err().expect_err_free(&json);
            assert_eq!(refused.code, super::super::status::UNIMPLEMENTED, "{json}");
            assert!(
                refused.message.contains(path),
                "{json}: {}",
                refused.message
            );
        }
    }

    trait ExpectErrFree {
        fn expect_err_free(self, json: &Json) -> Status;
    }

    impl ExpectErrFree for Option<Status> {
        fn expect_err_free(self, json: &Json) -> Status {
            self.unwrap_or_else(|| panic!("{json} was accepted"))
        }
    }

    #[test]
    fn malformed_queries_are_invalid_arguments() {
        for json in [
            json!({"searchRequestBody": {"query": {}}}),
            json!({"searchRequestBody": {"query": {"term": {"value": {"string": "v"}}}}}),
            json!({"searchRequestBody": {"query": {"term": {"field": "f"}}}}),
            json!({"searchRequestBody": {"query": {"term": {"field": "f", "value": {}}}}}),
            json!({"searchRequestBody": {"query": {"matchPhrase": {"field": "f"}}}}),
            json!({"searchRequestBody": {"query": {"prefix": {"field": "f"}}}}),
            json!({"searchRequestBody": {"query": {"exists": {}}}}),
            json!({"searchRequestBody": {"query": {"terms": {"terms": {}}}}}),
            json!({"searchRequestBody": {"query": {"terms": {"terms": {"a": {"value": {}}, "b": {"value": {}}}}}}}),
            json!({"searchRequestBody": {"query": {"range": {}}}}),
            json!({"searchRequestBody": {"query": {"range": {"numberRangeQuery": {}}}}}),
            json!({"searchRequestBody": {"sort": [{}]}}),
            json!({"searchRequestBody": {"query": {"range": {"numberRangeQuery": {"field": "n", "gt": "NaN"}}}}}),
        ] {
            let refused = request(&encode(json.clone())).err().expect_err_free(&json);
            assert_eq!(
                refused.code, INVALID_ARGUMENT,
                "{json}: {}",
                refused.message
            );
        }
    }

    #[test]
    fn the_covered_list_names_the_query_oneof_members_this_file_reads() {
        let desc = schema::message("QueryContainer");
        for name in COVERED_QUERIES {
            assert!(desc.get_field_by_name(name).is_some(), "{name}");
        }
    }
}
