//! A decoded call turned into the node's REST request, and its JSON answer into a response
//! message: the part every mapped method shares.

use bytes::Bytes;
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor, Value};

use super::fields::Fields;

use super::status::{INTERNAL, INVALID_ARGUMENT, OK, Status, from_rest};
use super::{call, respond};
use crate::stream::Shared;

/// The REST request a call stands for.
pub(crate) struct Rest {
    pub(crate) method: &'static str,
    pub(crate) target: String,
    pub(crate) body: Vec<u8>,
}

/// Decodes one request message of type `desc`.
pub(crate) fn decode(desc: MessageDescriptor, message: &[u8]) -> Result<DynamicMessage, Status> {
    DynamicMessage::decode(desc, message)
        .map_err(|_| Status::new(INVALID_ARGUMENT, "the request message does not decode"))
}

/// Appends `value` to `out`, percent-encoding everything but unreserved characters.
pub(crate) fn push_encoded(out: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in value.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
}

/// `?a=b&c=d`, built one parameter at a time.
#[derive(Default)]
pub(crate) struct Query(pub(crate) String);

impl Query {
    pub(crate) fn text(&mut self, name: &str, value: Option<&str>) {
        let Some(value) = value else { return };
        self.0.push(if self.0.is_empty() { '?' } else { '&' });
        self.0.push_str(name);
        self.0.push('=');
        push_encoded(&mut self.0, value);
    }

    pub(crate) fn flag(&mut self, name: &str, value: Option<bool>) {
        self.text(name, value.map(|b| if b { "true" } else { "false" }));
    }
}

pub(crate) fn join(items: &[Value]) -> Option<String> {
    // ALLOC: one request, within `grpc_max_message` and charged to the request-body pool at start.
    let words: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
    (!words.is_empty()).then(|| words.join(","))
}

/// `SourceConfigParam`: `fetch` is `_source=true|false`, `fields` is a list.
pub(crate) fn source_param(
    source: &DynamicMessage,
    path: &str,
    query: &mut Query,
) -> Result<(), Status> {
    let mut s = Fields::new(source, path);
    query.flag("_source", s.boolean("fetch"));
    if let Some(list) = s.message("fields") {
        let mut l = Fields::new(list, &s.at("fields"));
        query.text("_source", join(l.repeated("string_array")).as_deref());
        l.finish()?;
    }
    s.finish()
}

/// Sends `rest` to the node and returns its JSON answer.
///
/// A non-2xx answer is a gRPC error with the REST error JSON as its message.
pub(crate) async fn fetch(shared: &Shared, rest: &Rest) -> Result<serde_json::Value, Status> {
    let answer = call::unary(shared, rest.method, &rest.target, &rest.body).await?;
    let code = from_rest(answer.status);
    if code != OK {
        // ALLOC: the error text, at most `grpc_max_message`; `encode_message` cuts it at 4 KiB.
        let text = String::from_utf8_lossy(&answer.body).into_owned();
        return Err(Status::new(code, text));
    }
    serde_json::from_slice(&answer.body)
        .map_err(|_| Status::new(INTERNAL, "the node's answer is not JSON"))
}

/// Maps a JSON answer onto `response` and encodes it.
pub(crate) fn encode(
    json: &serde_json::Value,
    response: &MessageDescriptor,
) -> Result<Bytes, Status> {
    let message = respond::message(response, json, response.name())?;
    // ALLOC: the response message, about the size of the JSON answer, itself bounded by `grpc_max_message`.
    Ok(Bytes::from(message.encode_to_vec()))
}

/// Sends `rest` to the node and maps the answer onto `response`, encoded.
pub(crate) async fn roundtrip(
    shared: &Shared,
    rest: &Rest,
    response: &MessageDescriptor,
) -> Result<Bytes, Status> {
    encode(&fetch(shared, rest).await?, response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_characters_are_percent_encoded() {
        let mut s = String::new();
        push_encoded(&mut s, "a b/c,é~");
        assert_eq!(s, "a%20b%2Fc%2C%C3%A9~");
    }
}
