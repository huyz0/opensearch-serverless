//! The two XML documents the S3 client reads: a `ListObjectsV2` page,
//! parsed as it streams so no page is held whole, and the `<Code>` of an
//! error body.
//!
//! This is not a general XML parser. It reads elements by name, skips
//! declarations and attributes, and decodes the five predefined entities
//! and numeric character references, which is everything S3 emits in these
//! two documents. Every buffer has a fixed bound, and an element longer
//! than its bound is a [`BlobError::Protocol`] rather than a reason to grow.

use crate::error::{BlobError, BlobResult};
use crate::store::MAX_KEY_BYTES;

/// The longest element text kept: a key of [`MAX_KEY_BYTES`] whose every
/// byte is escaped as a six-byte entity, with room to spare.
pub const MAX_TEXT: usize = 8 * 1024;

/// The longest element name kept; longer names are read and ignored.
const MAX_NAME: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Between tags.
    Text,
    /// Just after `<`.
    Open,
    /// Reading a tag's name; `closing` for `</`.
    Name { closing: bool },
    /// Past a tag's name, up to its `>`.
    Rest { closing: bool },
    /// Inside `<?...>` or `<!...>`.
    Skip,
}

/// Which element's text is being kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    None,
    Key,
    Size,
    Truncated,
}

/// A streaming reader of one `ListObjectsV2` response page.
#[derive(Debug)]
pub struct ListParser {
    state: State,
    name: [u8; MAX_NAME],
    name_len: usize,
    /// The name ran past [`MAX_NAME`]; it matches nothing.
    name_long: bool,
    /// The previous byte was `/` inside a tag: `<a/>` opens and closes.
    slash: bool,
    field: Field,
    text: Vec<u8>,
    key: Vec<u8>,
    size: Option<u64>,
    has_key: bool,
    in_contents: bool,
    seen_root: bool,
    truncated: bool,
    keys: usize,
    max_keys: usize,
}

impl ListParser {
    /// A parser for a page of at most `max_keys` keys.
    pub fn new(max_keys: usize) -> Self {
        Self {
            state: State::Text,
            name: [0; MAX_NAME],
            name_len: 0,
            name_long: false,
            slash: false,
            field: Field::None,
            // ALLOC: MAX_TEXT bytes, once per LIST call.
            text: Vec::with_capacity(MAX_TEXT),
            // ALLOC: MAX_KEY_BYTES bytes, once per LIST call.
            key: Vec::with_capacity(MAX_KEY_BYTES),
            size: None,
            has_key: false,
            in_contents: false,
            seen_root: false,
            truncated: false,
            keys: 0,
            max_keys,
        }
    }

    /// Keys visited so far.
    pub fn keys(&self) -> usize {
        self.keys
    }

    /// Reads the next bytes of the document, visiting each object as its
    /// `</Contents>` arrives.
    pub fn feed(&mut self, bytes: &[u8], visit: &mut dyn FnMut(&str, u64)) -> BlobResult<()> {
        for &b in bytes {
            self.step(b, visit)?;
        }
        Ok(())
    }

    /// Ends the document: whether more keys follow this page.
    pub fn finish(&self) -> BlobResult<bool> {
        if !self.seen_root {
            return Err(BlobError::Protocol("not a ListBucketResult"));
        }
        if self.state != State::Text || self.in_contents {
            return Err(BlobError::Protocol("LIST document ended inside an element"));
        }
        Ok(self.truncated)
    }

    fn step(&mut self, b: u8, visit: &mut dyn FnMut(&str, u64)) -> BlobResult<()> {
        match self.state {
            State::Text if b == b'<' => self.state = State::Open,
            State::Text => {
                if self.field != Field::None {
                    if self.text.len() >= MAX_TEXT {
                        return Err(BlobError::Protocol("LIST element longer than its bound"));
                    }
                    self.text.push(b);
                }
            }
            State::Open => {
                self.name_len = 0;
                self.name_long = false;
                self.slash = false;
                self.state = match b {
                    b'/' => State::Name { closing: true },
                    b'?' | b'!' => State::Skip,
                    _ => {
                        self.push_name(b);
                        State::Name { closing: false }
                    }
                };
            }
            State::Name { closing } => match b {
                b'>' => self.end_tag(closing, visit)?,
                b' ' | b'\t' | b'\r' | b'\n' => self.state = State::Rest { closing },
                b'/' => {
                    self.slash = true;
                    self.state = State::Rest { closing };
                }
                _ => self.push_name(b),
            },
            State::Rest { closing } => match b {
                b'>' => self.end_tag(closing, visit)?,
                b'/' => self.slash = true,
                _ => self.slash = false,
            },
            State::Skip if b == b'>' => self.state = State::Text,
            State::Skip => {}
        }
        Ok(())
    }

    fn push_name(&mut self, b: u8) {
        match self.name.get_mut(self.name_len) {
            Some(slot) => {
                *slot = b;
                self.name_len = self.name_len.saturating_add(1);
            }
            None => self.name_long = true,
        }
    }

    fn name(&self) -> &[u8] {
        if self.name_long {
            return b"";
        }
        self.name.get(..self.name_len).unwrap_or_default()
    }

    fn end_tag(&mut self, closing: bool, visit: &mut dyn FnMut(&str, u64)) -> BlobResult<()> {
        self.state = State::Text;
        if closing {
            return self.close(visit);
        }
        self.open();
        if self.slash {
            // `<Name/>`: an empty element, opened and closed at once.
            self.close(visit)?;
        }
        Ok(())
    }

    fn open(&mut self) {
        let field = match self.name() {
            b"ListBucketResult" => {
                self.seen_root = true;
                Field::None
            }
            b"Contents" => {
                self.in_contents = true;
                self.has_key = false;
                self.size = None;
                Field::None
            }
            b"Key" if self.in_contents => Field::Key,
            b"Size" if self.in_contents => Field::Size,
            b"IsTruncated" if !self.in_contents => Field::Truncated,
            _ => Field::None,
        };
        if field != Field::None {
            self.text.clear();
            self.field = field;
        }
    }

    fn close(&mut self, visit: &mut dyn FnMut(&str, u64)) -> BlobResult<()> {
        match (self.name(), self.field) {
            (b"Key", Field::Key) => {
                decode(&self.text, &mut self.key)?;
                self.has_key = true;
            }
            (b"Size", Field::Size) => {
                let digits = std::str::from_utf8(&self.text)
                    .map_err(|_| BlobError::Protocol("LIST size is not a number"))?;
                self.size = Some(
                    digits
                        .trim()
                        .parse()
                        .map_err(|_| BlobError::Protocol("LIST size is not a number"))?,
                );
            }
            (b"IsTruncated", Field::Truncated) => {
                self.truncated = self.text.trim_ascii() == b"true";
            }
            (b"Contents", _) if self.in_contents => {
                self.in_contents = false;
                let (true, Some(size)) = (self.has_key, self.size) else {
                    return Err(BlobError::Protocol("LIST entry without a Key or a Size"));
                };
                if self.keys >= self.max_keys {
                    return Err(BlobError::Protocol("LIST page longer than max-keys"));
                }
                let key = std::str::from_utf8(&self.key)
                    .map_err(|_| BlobError::Protocol("LIST key is not UTF-8"))?;
                visit(key, size);
                self.keys = self.keys.saturating_add(1);
            }
            _ => {}
        }
        self.field = Field::None;
        Ok(())
    }
}

/// Decodes XML text into `out`: the five predefined entities and numeric
/// references. A key longer than [`MAX_KEY_BYTES`] once decoded, or an
/// entity not in that set, is refused.
pub fn decode(text: &[u8], out: &mut Vec<u8>) -> BlobResult<()> {
    out.clear();
    let mut rest = text;
    while let Some((&b, tail)) = rest.split_first() {
        if b == b'&' {
            let end = tail
                .iter()
                .position(|c| *c == b';')
                .ok_or(BlobError::Protocol("unterminated XML entity"))?;
            let (entity, after) = tail.split_at(end);
            let mut utf8 = [0u8; 4];
            let decoded: &[u8] = match entity {
                b"amp" => b"&",
                b"lt" => b"<",
                b"gt" => b">",
                b"quot" => b"\"",
                b"apos" => b"'",
                _ => char_ref(entity)?.encode_utf8(&mut utf8).as_bytes(),
            };
            push_bounded(out, decoded)?;
            rest = after.get(1..).unwrap_or_default();
        } else {
            push_bounded(out, &[b])?;
            rest = tail;
        }
    }
    Ok(())
}

fn push_bounded(out: &mut Vec<u8>, bytes: &[u8]) -> BlobResult<()> {
    if out.len().saturating_add(bytes.len()) > MAX_KEY_BYTES {
        return Err(BlobError::Protocol("LIST key longer than 1024 bytes"));
    }
    out.extend_from_slice(bytes);
    Ok(())
}

/// The character of a numeric reference body, `#65` or `#x41`.
fn char_ref(entity: &[u8]) -> BlobResult<char> {
    let bad = BlobError::Protocol("unknown XML entity");
    let digits = entity.strip_prefix(b"#").ok_or(bad.clone())?;
    let (digits, radix) = match digits.strip_prefix(b"x") {
        Some(hex) => (hex, 16),
        None => (digits, 10),
    };
    let text = std::str::from_utf8(digits).map_err(|_| bad.clone())?;
    let code = u32::from_str_radix(text, radix).map_err(|_| bad.clone())?;
    char::from_u32(code).ok_or(bad)
}

/// The `<Code>` of an S3 error body, if it has one in the bytes given.
pub fn error_code(body: &[u8]) -> Option<&str> {
    const OPEN: &[u8] = b"<Code>";
    let start = body
        .windows(OPEN.len())
        .position(|w| w == OPEN)?
        .checked_add(OPEN.len())?;
    let rest = body.get(start..)?;
    let end = rest.iter().position(|b| *b == b'<')?;
    std::str::from_utf8(rest.get(..end)?).ok().map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(doc: &[u8], max: usize, piece: usize) -> BlobResult<(Vec<(String, u64)>, bool)> {
        let mut p = ListParser::new(max);
        let mut seen = Vec::new();
        for chunk in doc.chunks(piece) {
            p.feed(chunk, &mut |k, s| seen.push((k.to_owned(), s)))?;
        }
        Ok((seen, p.finish()?))
    }

    const PAGE: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="urn:example:s3"><Name>b</Name><Prefix>p/</Prefix>
<KeyCount>2</KeyCount><MaxKeys>2</MaxKeys><IsTruncated>true</IsTruncated>
<Contents><Key>p/a&amp;b&lt;&gt;&quot;&apos;&#233;&#x41;</Key><LastModified>2026-11-15T00:00:00.000Z</LastModified>
<ETag>&quot;x&quot;</ETag><Size>12</Size><StorageClass>STANDARD</StorageClass></Contents>
<Contents><Key>p/z</Key><Size> 0 </Size><Owner/></Contents></ListBucketResult>"#;

    #[test]
    fn a_page_streams_its_keys_in_any_chunking() {
        for piece in [1, 2, 7, 64, PAGE.len()] {
            let (seen, truncated) = parse(PAGE, 2, piece).unwrap();
            assert_eq!(
                seen,
                [("p/a&b<>\"'éA".to_owned(), 12), ("p/z".to_owned(), 0)],
                "{piece}"
            );
            assert!(truncated);
        }
    }

    #[test]
    fn a_page_past_max_keys_is_refused() {
        assert_eq!(
            parse(PAGE, 1, 64),
            Err(BlobError::Protocol("LIST page longer than max-keys"))
        );
    }

    #[test]
    fn an_empty_final_page_is_not_truncated() {
        let doc = b"<ListBucketResult><IsTruncated>false</IsTruncated><KeyCount>0</KeyCount></ListBucketResult>";
        assert_eq!(parse(doc, 5, 3).unwrap(), (vec![], false));
    }

    #[test]
    fn documents_that_are_not_a_page_are_refused() {
        let cases: [(&[u8], &str); 7] = [
            (b"<Error><Code>X</Code></Error>", "not a ListBucketResult"),
            (
                b"<ListBucketResult><Contents><Key>a</Key></Contents>",
                "LIST entry without a Key or a Size",
            ),
            (
                b"<ListBucketResult><Contents><Key>a</Key><Size>x</Size>",
                "LIST size is not a number",
            ),
            (
                b"<ListBucketResult><Contents><Key>a</Key><Size>1</Size>",
                "LIST document ended inside an element",
            ),
            (
                b"<ListBucketResult><Contents><Key>a&bogus;</Key>",
                "unknown XML entity",
            ),
            (
                b"<ListBucketResult><Contents><Key>a&amp</Key>",
                "unterminated XML entity",
            ),
            (
                b"<ListBucketResult><Contents><Key>\xff</Key><Size>1</Size></Contents>",
                "LIST key is not UTF-8",
            ),
        ];
        for (doc, why) in cases {
            assert_eq!(
                parse(doc, 5, 4).map(|_| ()),
                Err(BlobError::Protocol(why)),
                "{why}"
            );
        }
    }

    #[test]
    fn a_size_that_is_not_utf8_is_refused() {
        let doc = b"<ListBucketResult><Contents><Key>a</Key><Size>\xff</Size>";
        assert_eq!(
            parse(doc, 5, 4).map(|_| ()),
            Err(BlobError::Protocol("LIST size is not a number"))
        );
    }

    #[test]
    fn element_text_is_bounded() {
        let mut doc = b"<ListBucketResult><Contents><Key>".to_vec();
        doc.extend(std::iter::repeat_n(b'k', MAX_TEXT));
        let mut at_limit = doc.clone();
        at_limit.extend_from_slice(b"</Key>");
        // At the text bound the element is read, then refused as a key.
        assert_eq!(
            parse(&at_limit, 5, 512).map(|_| ()),
            Err(BlobError::Protocol("LIST key longer than 1024 bytes"))
        );
        doc.push(b'k');
        assert_eq!(
            parse(&doc, 5, 512).map(|_| ()),
            Err(BlobError::Protocol("LIST element longer than its bound"))
        );
    }

    #[test]
    fn a_key_of_1024_bytes_decodes_and_one_more_does_not() {
        let mut out = Vec::new();
        let longest = "&amp;".repeat(MAX_KEY_BYTES);
        decode(longest.as_bytes(), &mut out).unwrap();
        assert_eq!(out.len(), MAX_KEY_BYTES);
        let over = format!("{longest}x");
        assert!(decode(over.as_bytes(), &mut out).is_err());
    }

    #[test]
    fn numeric_references_must_name_a_character() {
        let mut out = Vec::new();
        for bad in [&b"&#xD800;"[..], b"&#;", b"&#xzz;", b"&#\xff;"] {
            assert_eq!(
                decode(bad, &mut out),
                Err(BlobError::Protocol("unknown XML entity"))
            );
        }
    }

    #[test]
    fn long_names_and_attributes_are_skipped() {
        let doc = b"<ListBucketResult a='1'><AVeryLongElementNameThatRunsPastThirtyTwoBytes>x</AVeryLongElementNameThatRunsPastThirtyTwoBytes><IsTruncated\n>true</IsTruncated ></ListBucketResult>";
        assert_eq!(parse(doc, 5, 5).unwrap(), (vec![], true));
    }

    #[test]
    fn the_code_of_an_error_body() {
        let body = b"<?xml version=\"1.0\"?><Error><Code>ConditionalRequestConflict</Code><Message>m</Message></Error>";
        assert_eq!(error_code(body), Some("ConditionalRequestConflict"));
        assert_eq!(error_code(b"<Error></Error>"), None);
        assert_eq!(error_code(b"<Code>unterminated"), None);
        assert_eq!(error_code(b"<Code>\xff</Code>"), None);
    }
}
