//! `DocumentService/Bulk`: a `BulkRequest` as a REST `_bulk` call (NDJSON) and its answer as a
//! `BulkResponse`. Only the covered subset is read; any other field set is refused by path.

use prost_reflect::{DynamicMessage, Value};
use serde_json::{Map, Value as Json, json};

use super::fields::Fields;
use super::rest::{Query, Rest, decode, join, push_encoded, source_param};
use super::schema;
use super::status::{INVALID_ARGUMENT, Status};

/// Decodes `message` and maps it to the REST request.
pub(crate) fn request(message: &[u8]) -> Result<Rest, Status> {
    let msg = decode(schema::message("BulkRequest"), message)?;
    map(&msg)
}

fn map(msg: &DynamicMessage) -> Result<Rest, Status> {
    let mut f = Fields::new(msg, "BulkRequest");
    let mut target = String::from("/");
    if let Some(index) = f.string("index") {
        push_encoded(&mut target, index);
        target.push('/');
    }
    target.push_str("_bulk");
    let mut query = Query::default();
    query.text("routing", f.string("routing"));
    query.text("pipeline", f.string("pipeline"));
    query.text("timeout", f.string("timeout"));
    query.text("refresh", f.enum_word("refresh").as_deref());
    query.flag("require_alias", f.boolean("require_alias"));
    if let Some(source) = f.message("x_source") {
        source_param(source, "BulkRequest.x_source", &mut query)?;
    }
    query.text(
        "_source_excludes",
        join(f.repeated("x_source_excludes")).as_deref(),
    );
    query.text(
        "_source_includes",
        join(f.repeated("x_source_includes")).as_deref(),
    );
    // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
    let mut body = Vec::new();
    for (i, item) in f.repeated("bulk_request_body").iter().enumerate() {
        let Value::Message(item) = item else { continue };
        // ALLOC: an item path, a few bytes.
        item_lines(
            item,
            &format!("BulkRequest.bulk_request_body[{i}]"),
            &mut body,
        )?;
    }
    f.finish()?;
    target.push_str(&query.0);
    Ok(Rest {
        method: "POST",
        target,
        body,
    })
}

/// The action line, and the source line the action needs, of one bulk item.
fn item_lines(item: &DynamicMessage, path: &str, out: &mut Vec<u8>) -> Result<(), Status> {
    let mut f = Fields::new(item, path);
    let container = f.message("operation_container").ok_or_else(|| {
        Status::new(
            INVALID_ARGUMENT,
            // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
            format!("{path}.operation_container is required"),
        )
    })?;
    let mut c = Fields::new(container, &f.at("operation_container"));
    let mut action = None;
    for name in ["index", "create", "update", "delete"] {
        let Some(op) = c.message(name) else { continue };
        let mut o = Fields::new(op, &c.at(name));
        let mut meta = Map::new();
        for (field, key) in [
            ("x_id", "_id"),
            ("x_index", "_index"),
            ("routing", "routing"),
        ] {
            if let Some(v) = o.string(field) {
                // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                meta.insert(key.to_owned(), json!(v));
            }
        }
        o.finish()?;
        action = Some((name, meta));
    }
    c.finish()?;
    let (name, meta) =
        // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
        action.ok_or_else(|| Status::new(INVALID_ARGUMENT, format!("{path} has no operation")))?;
    line(out, &json!({ name: meta }))?;
    match name {
        "delete" => {}
        "update" => {
            let action = f.message("update_action").ok_or_else(|| {
                Status::new(
                    INVALID_ARGUMENT,
                    // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                    format!("{path}.update_action is required"),
                )
            })?;
            let mut u = Fields::new(action, &f.at("update_action"));
            let mut doc = Map::new();
            for (field, key) in [("doc", "doc"), ("upsert", "upsert")] {
                if let Some(bytes) = u.bytes(field) {
                    let parsed = serde_json::from_slice(bytes).map_err(|_| {
                        // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                        Status::new(INVALID_ARGUMENT, format!("{} is not JSON", u.at(field)))
                    })?;
                    // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                    doc.insert(key.to_owned(), parsed);
                }
            }
            for (field, key) in [
                ("doc_as_upsert", "doc_as_upsert"),
                ("detect_noop", "detect_noop"),
            ] {
                if let Some(b) = u.boolean(field) {
                    // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                    doc.insert(key.to_owned(), json!(b));
                }
            }
            u.finish()?;
            line(out, &Json::Object(doc))?;
        }
        _ => {
            let source = f.bytes("object").ok_or_else(|| {
                // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
                Status::new(INVALID_ARGUMENT, format!("{path}.object is required"))
            })?;
            // A line break outside a string is white space in JSON and inside one is escaped, so
            // replacing raw ones keeps the document as it was and the NDJSON line whole.
            out.extend(
                source
                    .iter()
                    .map(|&b| if b == b'\n' || b == b'\r' { b' ' } else { b }),
            );
            out.push(b'\n');
        }
    }
    f.finish()
}

fn line(out: &mut Vec<u8>, value: &Json) -> Result<(), Status> {
    serde_json::to_writer(&mut *out, value)
        .map_err(|_| Status::new(INVALID_ARGUMENT, "an action does not encode"))?;
    out.push(b'\n');
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn bulk(json: &str) -> Vec<u8> {
        let desc = schema::message("BulkRequest");
        let msg = DynamicMessage::deserialize(desc, &mut serde_json::Deserializer::from_str(json))
            .expect("protojson");
        msg.encode_to_vec()
    }

    fn body(rest: &Rest) -> String {
        String::from_utf8(rest.body.clone()).expect("utf8")
    }

    #[test]
    fn the_four_actions_become_ndjson() {
        let m = bulk(
            r#"{"index":"a b","routing":"r","refresh":"REFRESH_WAIT_FOR","requireAlias":true,
            "bulkRequestBody":[
             {"operationContainer":{"index":{"xId":"1"}},"object":"eyJ4IjoxfQ=="},
             {"operationContainer":{"create":{"xId":"2","routing":"k"}},"object":"eyJ4IjoyfQ=="},
             {"operationContainer":{"update":{"xId":"3"}},"updateAction":{"doc":"eyJ4IjozfQ==","docAsUpsert":true}},
             {"operationContainer":{"delete":{"xId":"4"}}}]}"#,
        );
        let rest = request(&m).expect("maps");
        assert_eq!(rest.method, "POST");
        assert_eq!(
            rest.target,
            "/a%20b/_bulk?routing=r&refresh=wait_for&require_alias=true"
        );
        assert_eq!(
            body(&rest),
            "{\"index\":{\"_id\":\"1\"}}\n{\"x\":1}\n\
             {\"create\":{\"_id\":\"2\",\"routing\":\"k\"}}\n{\"x\":2}\n\
             {\"update\":{\"_id\":\"3\"}}\n{\"doc\":{\"x\":3},\"doc_as_upsert\":true}\n\
             {\"delete\":{\"_id\":\"4\"}}\n"
        );
    }

    #[test]
    fn a_document_with_line_breaks_stays_one_line() {
        // {"a":\n1}\r\n
        let m = bulk(
            r#"{"bulkRequestBody":[{"operationContainer":{"index":{}},"object":"eyJhIjoKMX0NCg=="}]}"#,
        );
        let rest = request(&m).expect("maps");
        assert_eq!(rest.target, "/_bulk");
        assert_eq!(body(&rest), "{\"index\":{}}\n{\"a\": 1}  \n");
    }

    #[test]
    fn source_parameters_are_mapped() {
        let m = bulk(
            r#"{"xSource":{"fields":{"stringArray":["a","b"]}},"xSourceIncludes":["i"],"xSourceExcludes":["e","f"],"pipeline":"p","timeout":"5s"}"#,
        );
        let rest = request(&m).expect("maps");
        assert_eq!(
            rest.target,
            "/_bulk?pipeline=p&timeout=5s&_source=a%2Cb&_source_excludes=e%2Cf&_source_includes=i"
        );
        let m = bulk(r#"{"xSource":{"fetch":false}}"#);
        assert_eq!(request(&m).expect("maps").target, "/_bulk?_source=false");
    }

    #[test]
    fn fields_outside_the_subset_are_refused_by_path() {
        for (json, path) in [
            (r#"{"type":"t"}"#, "BulkRequest.type"),
            (
                r#"{"waitForActiveShards":{"count":2}}"#,
                "BulkRequest.wait_for_active_shards",
            ),
            (
                r#"{"bulkRequestBody":[{"operationContainer":{"index":{"version":"3"}},"object":"e30="}]}"#,
                "BulkRequest.bulk_request_body[0].operation_container.index.version",
            ),
            (
                r#"{"bulkRequestBody":[{"operationContainer":{"update":{}},"updateAction":{"scriptedUpsert":true}}]}"#,
                "BulkRequest.bulk_request_body[0].update_action.scripted_upsert",
            ),
            (
                r#"{"bulkRequestBody":[{"operationContainer":{"delete":{}},"object":"e30="}]}"#,
                "BulkRequest.bulk_request_body[0].object",
            ),
        ] {
            let refused = request(&bulk(json)).err().expect(json);
            assert_eq!(refused.code, super::super::status::UNIMPLEMENTED, "{json}");
            assert!(
                refused.message.contains(path),
                "{json}: {}",
                refused.message
            );
        }
    }

    #[test]
    fn malformed_items_are_invalid_arguments() {
        for json in [
            r#"{"bulkRequestBody":[{}]}"#,
            r#"{"bulkRequestBody":[{"operationContainer":{}}]}"#,
            r#"{"bulkRequestBody":[{"operationContainer":{"index":{}}}]}"#,
            r#"{"bulkRequestBody":[{"operationContainer":{"update":{}}}]}"#,
            r#"{"bulkRequestBody":[{"operationContainer":{"update":{}},"updateAction":{"doc":"bm90IGpzb24="}}]}"#,
        ] {
            let refused = request(&bulk(json)).err().expect(json);
            assert_eq!(refused.code, INVALID_ARGUMENT, "{json}");
        }
        assert_eq!(
            request(&[0xff]).err().map(|s| s.code),
            Some(INVALID_ARGUMENT)
        );
    }
}
