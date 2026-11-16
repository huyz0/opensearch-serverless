//! A JSON writer into a caller's buffer, escaping strings as the shell's
//! Jackson does.
//!
//! The shell writes its records with OpenSearch's `XContentBuilder` over
//! Jackson 3, configured in `JsonXContent`: compact, every property name
//! quoted, strings escaped by `JsonGenerator.writeString`'s defaults. Those
//! escape `"` and `\`, write `\b \t \n \f \r` short and every other control
//! below U+0020 as `\u00XX` in upper case, and leave `/` and everything from
//! U+007F up as raw UTF-8 (`fixtures/jackson/every-character.json`).
//!
//! Writing past the buffer's end stops writing but keeps counting, so a
//! caller told the buffer is short also learns how long one must be.

use crate::error::{FormatError, len64};

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// A JSON writer over a fixed buffer.
pub(crate) struct Out<'o> {
    buf: &'o mut [u8],
    // Bytes the document has taken so far, written or not.
    len: usize,
    // Whether a value has been written in the open container, so the next
    // one needs a comma.
    comma: bool,
}

impl<'o> Out<'o> {
    /// A writer filling `buf` from its start.
    pub(crate) fn new(buf: &'o mut [u8]) -> Self {
        Self {
            buf,
            len: 0,
            comma: false,
        }
    }

    /// Appends bytes as they are.
    pub(crate) fn raw(&mut self, bytes: &[u8]) {
        let end = self.len.saturating_add(bytes.len());
        if let Some(room) = self.buf.get_mut(self.len..end) {
            room.copy_from_slice(bytes);
        }
        self.len = end;
    }

    fn byte(&mut self, b: u8) {
        self.raw(&[b]);
    }

    fn separate(&mut self) {
        if self.comma {
            self.byte(b',');
        }
        self.comma = true;
    }

    /// Opens an object as a value.
    pub(crate) fn start_object(&mut self) {
        self.separate();
        self.byte(b'{');
        self.comma = false;
    }

    /// Closes the open object or array.
    pub(crate) fn end_object(&mut self) {
        self.byte(b'}');
        self.comma = true;
    }

    /// Opens an array under a field name.
    pub(crate) fn start_array_field(&mut self, name: &str) {
        self.field(name);
        self.byte(b'[');
        self.comma = false;
    }

    /// Closes the open array.
    pub(crate) fn end_array(&mut self) {
        self.byte(b']');
        self.comma = true;
    }

    /// A field name and its colon; the value follows.
    pub(crate) fn field(&mut self, name: &str) {
        self.separate();
        self.quoted(name);
        self.byte(b':');
        // The value that follows takes no comma of its own.
        self.comma = false;
    }

    /// A string value, escaped.
    pub(crate) fn string(&mut self, text: &str) {
        self.separate();
        self.quoted(text);
    }

    /// A string field.
    pub(crate) fn string_field(&mut self, name: &str, text: &str) {
        self.field(name);
        self.string(text);
    }

    /// An integer field.
    pub(crate) fn int_field(&mut self, name: &str, n: i64) {
        self.field(name);
        self.separate();
        self.int(n);
    }

    /// A boolean field.
    pub(crate) fn bool_field(&mut self, name: &str, value: bool) {
        self.field(name);
        self.separate();
        self.raw(if value { b"true" } else { b"false" });
    }

    /// A field whose value is `null`.
    pub(crate) fn null_field(&mut self, name: &str) {
        self.field(name);
        self.separate();
        self.raw(b"null");
    }

    /// Opens an object under a field name.
    pub(crate) fn start_object_field(&mut self, name: &str) {
        self.field(name);
        self.start_object();
    }

    /// An integer, in decimal, with no separator: for a text record.
    pub(crate) fn int(&mut self, n: i64) {
        // ALLOC: none; at most 20 digits and a sign, on the stack.
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        let mut rest = n.unsigned_abs();
        loop {
            at = at.saturating_sub(1);
            if let Some(slot) = digits.get_mut(at) {
                // The remainder of a division by 10 is a digit.
                *slot = b'0'.saturating_add(u8::try_from(rest % 10).unwrap_or(0));
            }
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        if n < 0 {
            self.byte(b'-');
        }
        self.raw(digits.get(at..).unwrap_or_default());
    }

    /// A field whose value is already JSON, written as it is.
    pub(crate) fn raw_field(&mut self, name: &str, json: &[u8]) {
        self.field(name);
        self.separate();
        self.raw(json);
    }

    fn quoted(&mut self, text: &str) {
        self.byte(b'"');
        let bytes = text.as_bytes();
        let mut plain = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let short = match b {
                b'"' => b'"',
                b'\\' => b'\\',
                0x08 => b'b',
                b'\t' => b't',
                b'\n' => b'n',
                0x0c => b'f',
                b'\r' => b'r',
                0..=0x1f => 0,
                _ => continue,
            };
            self.raw(bytes.get(plain..i).unwrap_or_default());
            plain = i.saturating_add(1);
            if short == 0 {
                let hex = |nibble: u8| HEX.get(usize::from(nibble)).copied().unwrap_or(b'0');
                self.raw(&[b'\\', b'u', b'0', b'0', hex(b >> 4), hex(b & 0xf)]);
            } else {
                self.raw(&[b'\\', short]);
            }
        }
        self.raw(bytes.get(plain..).unwrap_or_default());
        self.byte(b'"');
    }

    /// The length written, or how long a buffer the document needs when it
    /// did not fit.
    ///
    /// # Errors
    ///
    /// [`FormatError::TooLarge`] when the document is longer than the
    /// buffer: `actual` is the length it needs.
    pub(crate) fn finish(self, what: &'static str) -> Result<usize, FormatError> {
        if self.len > self.buf.len() {
            return Err(FormatError::TooLarge {
                what,
                limit: len64(self.buf.len()),
                actual: len64(self.len),
            });
        }
        Ok(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(f: impl FnOnce(&mut Out<'_>)) -> String {
        let mut buf = [0u8; 256];
        let mut out = Out::new(&mut buf);
        f(&mut out);
        let n = out.finish("test").unwrap();
        String::from_utf8(buf[..n].to_vec()).unwrap()
    }

    #[test]
    fn integers_print_as_java_longs() {
        for (n, text) in [
            (0, "0"),
            (7, "7"),
            (10, "10"),
            (-1, "-1"),
            (i64::MAX, "9223372036854775807"),
            (i64::MIN, "-9223372036854775808"),
        ] {
            assert_eq!(written(|o| o.int_field("n", n)), format!("\"n\":{text}"));
        }
    }

    #[test]
    fn values_are_separated_by_commas() {
        let json = written(|o| {
            o.start_object();
            o.string_field("a", "x");
            o.int_field("b", 1);
            o.start_array_field("c");
            o.string("p");
            o.string("q");
            o.end_array();
            o.field("d");
            o.start_object();
            o.end_object();
            o.raw_field("e", b"{}");
            o.end_object();
        });
        assert_eq!(json, r#"{"a":"x","b":1,"c":["p","q"],"d":{},"e":{}}"#);
    }

    #[test]
    fn a_short_buffer_reports_the_length_needed() {
        let mut buf = [0u8; 4];
        let mut out = Out::new(&mut buf);
        out.string("hello");
        assert_eq!(
            out.finish("test"),
            Err(FormatError::TooLarge {
                what: "test",
                limit: 4,
                actual: 7
            })
        );
        let mut exact = [0u8; 7];
        let mut out = Out::new(&mut exact);
        out.string("hello");
        assert_eq!(out.finish("test"), Ok(7));
        assert_eq!(&exact, b"\"hello\"");
    }
}
