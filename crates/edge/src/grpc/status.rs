//! gRPC status codes, the REST status table, and the percent-encoding of `grpc-message`.

/// A gRPC status: its code and a message for the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) code: u8,
    pub(crate) message: String,
}

pub(crate) const OK: u8 = 0;
pub(crate) const CANCELLED: u8 = 1;
pub(crate) const INVALID_ARGUMENT: u8 = 3;
pub(crate) const DEADLINE_EXCEEDED: u8 = 4;
pub(crate) const NOT_FOUND: u8 = 5;
pub(crate) const PERMISSION_DENIED: u8 = 7;
pub(crate) const RESOURCE_EXHAUSTED: u8 = 8;
pub(crate) const FAILED_PRECONDITION: u8 = 9;
pub(crate) const ABORTED: u8 = 10;
pub(crate) const UNIMPLEMENTED: u8 = 12;
pub(crate) const INTERNAL: u8 = 13;
pub(crate) const UNAVAILABLE: u8 = 14;
pub(crate) const UNAUTHENTICATED: u8 = 16;

/// The longest `grpc-message` sent, in bytes.
const MAX_MESSAGE: usize = 4096;

impl Status {
    pub(crate) fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// The gRPC code for a REST status (the table in docs/specs/grpc-front/SPEC.md).
pub(crate) fn from_rest(status: u16) -> u8 {
    match status {
        200..=299 => OK,
        400 => INVALID_ARGUMENT,
        401 => UNAUTHENTICATED,
        403 => PERMISSION_DENIED,
        404 => NOT_FOUND,
        409 => ABORTED,
        413 | 429 => RESOURCE_EXHAUSTED,
        499 => CANCELLED,
        501 => UNIMPLEMENTED,
        503 => UNAVAILABLE,
        504 => DEADLINE_EXCEEDED,
        s if (400..500).contains(&s) => FAILED_PRECONDITION,
        _ => INTERNAL,
    }
}

/// `grpc-message` is percent-encoded: bytes outside printable ASCII, and `%`, become `%XX`.
/// The message is cut at 4 KiB of encoded text, on a byte boundary of the original.
pub(crate) fn encode_message(message: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    // ALLOC: at most MAX_MESSAGE bytes, checked below.
    let mut out = String::with_capacity(message.len().min(MAX_MESSAGE));
    for &b in message.as_bytes() {
        let need = if (0x20..0x7f).contains(&b) && b != b'%' {
            1
        } else {
            3
        };
        if out.len().saturating_add(need) > MAX_MESSAGE {
            break;
        }
        if need == 1 {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_statuses_map_as_the_spec_table() {
        let table = [
            (200, OK),
            (400, INVALID_ARGUMENT),
            (401, UNAUTHENTICATED),
            (403, PERMISSION_DENIED),
            (404, NOT_FOUND),
            (409, ABORTED),
            (413, RESOURCE_EXHAUSTED),
            (429, RESOURCE_EXHAUSTED),
            (499, CANCELLED),
            (500, INTERNAL),
            (501, UNIMPLEMENTED),
            (503, UNAVAILABLE),
            (504, DEADLINE_EXCEEDED),
            (418, FAILED_PRECONDITION),
            (502, INTERNAL),
        ];
        for (rest, grpc) in table {
            assert_eq!(from_rest(rest), grpc, "{rest}");
        }
    }

    #[test]
    fn messages_are_percent_encoded_and_bounded() {
        assert_eq!(encode_message("a b%c\n\u{e9}"), "a b%25c%0A%C3%A9");
        assert_eq!(encode_message(&"x".repeat(10_000)).len(), MAX_MESSAGE);
        assert!(encode_message(&"\u{e9}".repeat(10_000)).len() <= MAX_MESSAGE);
    }
}
