//! `SearchService/CreatePit` and `DeletePit`: point-in-time requests as REST calls.

use prost_reflect::Value;
use serde_json::json;

use super::fields::Fields;
use super::rest::{Query, Rest, decode, push_encoded};
use super::schema;
use super::status::{INVALID_ARGUMENT, Status};

/// `CreatePitRequest { index, keep_alive }` as `POST /{index}/_search/point_in_time`.
pub(crate) fn create(message: &[u8]) -> Result<Rest, Status> {
    let msg = decode(schema::message("CreatePitRequest"), message)?;
    let mut f = Fields::new(&msg, "CreatePitRequest");
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
    target.push_str("_search/point_in_time");
    let keep_alive = f
        .string("keep_alive")
        .filter(|k| !k.is_empty())
        .ok_or_else(|| Status::new(INVALID_ARGUMENT, "CreatePitRequest.keep_alive is required"))?;
    let mut query = Query::default();
    query.text("keep_alive", Some(keep_alive));
    f.finish()?;
    target.push_str(&query.0);
    Ok(Rest {
        method: "POST",
        target,
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        body: Vec::new(),
    })
}

/// `DeletePitRequest { pit_id }` as `DELETE /_search/point_in_time` with the ids as the body.
pub(crate) fn delete(message: &[u8]) -> Result<Rest, Status> {
    let msg = decode(schema::message("DeletePitRequest"), message)?;
    let mut f = Fields::new(&msg, "DeletePitRequest");
    let ids: Vec<&str> = f
        .repeated("pit_id")
        .iter()
        .filter_map(Value::as_str)
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        .collect();
    f.finish()?;
    if ids.is_empty() {
        return Err(Status::new(
            INVALID_ARGUMENT,
            "DeletePitRequest.pit_id is required",
        ));
    }
    let body = serde_json::to_vec(&json!({ "pit_id": ids }))
        .map_err(|_| Status::new(INVALID_ARGUMENT, "the pit ids do not encode"))?;
    Ok(Rest {
        method: "DELETE",
        // ALLOC: one request or answer, within `grpc_max_message` and charged to the request-body pool at start.
        target: "/_search/point_in_time".to_owned(),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use prost_reflect::DynamicMessage;
    use serde_json::{Value as Json, json};

    fn encode(name: &str, json: Json) -> Vec<u8> {
        DynamicMessage::deserialize(schema::message(name), json)
            .expect("protojson")
            .encode_to_vec()
    }

    #[test]
    fn create_is_a_post_with_the_keep_alive() {
        let rest = create(&encode(
            "CreatePitRequest",
            json!({"index": ["a b", "c"], "keepAlive": "1m"}),
        ))
        .expect("maps");
        assert_eq!(rest.method, "POST");
        assert_eq!(rest.target, "/a%20b,c/_search/point_in_time?keep_alive=1m");
        assert!(rest.body.is_empty());
        let rest = create(&encode("CreatePitRequest", json!({"keepAlive": "5s"}))).expect("maps");
        assert_eq!(rest.target, "/_search/point_in_time?keep_alive=5s");
    }

    #[test]
    fn create_needs_a_keep_alive_and_refuses_the_rest() {
        let none = create(&encode("CreatePitRequest", json!({"index": ["a"]})))
            .err()
            .expect("refused");
        assert_eq!(none.code, INVALID_ARGUMENT);
        let empty = create(&encode("CreatePitRequest", json!({"keepAlive": ""})))
            .err()
            .expect("refused");
        assert_eq!(empty.code, INVALID_ARGUMENT);
        let refused = create(&encode(
            "CreatePitRequest",
            json!({"keepAlive": "1m", "preference": "p"}),
        ))
        .err()
        .expect("refused");
        assert_eq!(refused.code, super::super::status::UNIMPLEMENTED);
        assert!(refused.message.contains("CreatePitRequest.preference"));
        assert_eq!(
            create(&[0xff]).err().map(|s| s.code),
            Some(INVALID_ARGUMENT)
        );
    }

    #[test]
    fn delete_sends_the_ids_as_the_body() {
        let rest = delete(&encode("DeletePitRequest", json!({"pitId": ["x", "y"]}))).expect("maps");
        assert_eq!(
            (rest.method, rest.target.as_str()),
            ("DELETE", "/_search/point_in_time")
        );
        let body: Json = serde_json::from_slice(&rest.body).expect("json");
        assert_eq!(body, json!({"pit_id": ["x", "y"]}));
        let none = delete(&encode("DeletePitRequest", json!({})))
            .err()
            .expect("refused");
        assert_eq!(none.code, INVALID_ARGUMENT);
        assert_eq!(
            delete(&[0xff]).err().map(|s| s.code),
            Some(INVALID_ARGUMENT)
        );
    }
}
