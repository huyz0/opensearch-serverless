//! A decoded call turned into the node's REST request, and its JSON answer into a response
//! message: the part every mapped method shares.

use bytes::Bytes;
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor};

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

/// Sends `rest` to the node and maps the answer onto `response`, encoded.
///
/// A non-2xx answer is a gRPC error with the REST error JSON as its message.
pub(crate) async fn roundtrip(
    shared: &Shared,
    rest: &Rest,
    response: &MessageDescriptor,
) -> Result<Bytes, Status> {
    let answer = call::unary(shared, rest.method, &rest.target, &rest.body).await?;
    let code = from_rest(answer.status);
    if code != OK {
        // ALLOC: the error text, at most `grpc_max_message`; `encode_message` cuts it at 4 KiB.
        let text = String::from_utf8_lossy(&answer.body).into_owned();
        return Err(Status::new(code, text));
    }
    let json: serde_json::Value = serde_json::from_slice(&answer.body)
        .map_err(|_| Status::new(INTERNAL, "the node's answer is not JSON"))?;
    let message = respond::message(response, &json, response.name())?;
    // ALLOC: the response message, about the size of the JSON answer, itself bounded by `grpc_max_message`.
    Ok(Bytes::from(message.encode_to_vec()))
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
