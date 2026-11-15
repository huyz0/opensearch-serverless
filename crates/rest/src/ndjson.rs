//! Splitting a newline-delimited body (`_bulk`, `_msearch`) into lines as it
//! streams.
//!
//! Each line comes out as one or more parts, slices of the chunk fed in, so
//! the splitter copies and holds nothing: a line of any length passes through
//! in pieces no larger than the body chunks. A line's parts feed straight
//! into a [`crate::json::Tokenizer`], which is reset at each line's end.
//!
//! Ported from `BulkRequestParser#findNextMarker` and `#sliceTrimmingCarriageReturn`
//! (server/src/main/java/org/opensearch/action/bulk/BulkRequestParser.java at
//! huyz0/OpenSearch@49f238ed): lines end at `\n`, a `\r` just before it is
//! dropped, and bytes after the last `\n` are refused with Java's message.

use crate::error::RestError;

/// Receives lines in parts.
pub trait LineSink {
    /// The sink's error.
    type Error: From<RestError>;

    /// The next part of the current line (never empty).
    fn part(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;

    /// The current line, 1-based, has ended.
    fn end_line(&mut self, line: u64) -> Result<(), Self::Error>;
}

/// Splits a stream of chunks into lines.
#[derive(Debug, Default)]
pub struct Splitter {
    line: u64,
    open: bool,
    pending_cr: bool,
}

impl Splitter {
    /// A splitter at the start of a body.
    pub fn new() -> Self {
        Self {
            line: 1,
            ..Self::default()
        }
    }

    /// The 1-based number of the line being read.
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Feeds the next chunk of the body.
    pub fn feed<S: LineSink>(&mut self, mut chunk: &[u8], sink: &mut S) -> Result<(), S::Error> {
        while !chunk.is_empty() {
            match chunk.iter().position(|&b| b == b'\n') {
                Some(nl) => {
                    let mut part = &chunk[..nl];
                    if part.is_empty() {
                        // A `\r` held back from the previous chunk ends here.
                        self.pending_cr = false;
                    } else {
                        self.flush_cr(sink)?;
                        part = part.strip_suffix(b"\r").unwrap_or(part);
                        if !part.is_empty() {
                            sink.part(part)?;
                        }
                    }
                    sink.end_line(self.line)?;
                    self.line = self.line.saturating_add(1);
                    self.open = false;
                    chunk = &chunk[nl.saturating_add(1)..];
                }
                None => {
                    self.flush_cr(sink)?;
                    self.open = true;
                    let (part, cr) = match chunk.strip_suffix(b"\r") {
                        Some(p) => (p, true),
                        None => (chunk, false),
                    };
                    if !part.is_empty() {
                        sink.part(part)?;
                    }
                    self.pending_cr = cr;
                    chunk = &[];
                }
            }
        }
        Ok(())
    }

    /// Ends the body: refused when bytes follow the last newline.
    pub fn finish(&mut self) -> Result<(), RestError> {
        if self.open || self.pending_cr {
            return Err(RestError::illegal_argument(
                "The bulk request must be terminated by a newline [\\n]",
            ));
        }
        Ok(())
    }

    /// A `\r` held back at a chunk's end was not before a `\n`: it is data.
    fn flush_cr<S: LineSink>(&mut self, sink: &mut S) -> Result<(), S::Error> {
        if std::mem::take(&mut self.pending_cr) {
            sink.part(b"\r")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Lines {
        current: Vec<u8>,
        lines: Vec<(u64, String)>,
        max_part: usize,
    }

    impl LineSink for Lines {
        type Error = RestError;
        fn part(&mut self, bytes: &[u8]) -> Result<(), RestError> {
            assert!(!bytes.is_empty());
            self.max_part = self.max_part.max(bytes.len());
            self.current.extend_from_slice(bytes);
            Ok(())
        }
        fn end_line(&mut self, line: u64) -> Result<(), RestError> {
            let s = String::from_utf8(std::mem::take(&mut self.current)).expect("utf-8");
            self.lines.push((line, s));
            Ok(())
        }
    }

    fn split(body: &[u8], feed: usize) -> Result<Lines, RestError> {
        let mut s = Splitter::new();
        let mut sink = Lines::default();
        for chunk in body.chunks(feed) {
            s.feed(chunk, &mut sink)?;
        }
        s.finish()?;
        Ok(sink)
    }

    fn texts(l: &Lines) -> Vec<&str> {
        l.lines.iter().map(|(_, s)| s.as_str()).collect()
    }

    #[test]
    fn lines_split_at_newlines_in_any_feed_size() {
        let body = b"{\"index\":{}}\r\n{\"a\":1}\n\n{\"b\":\"x\ry\"}\r\n";
        for feed in 1..body.len() {
            let l = split(body, feed).expect("ok");
            assert_eq!(
                texts(&l),
                ["{\"index\":{}}", "{\"a\":1}", "", "{\"b\":\"x\ry\"}"],
                "feed {feed}"
            );
            assert_eq!(
                l.lines.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                [1, 2, 3, 4]
            );
            assert!(l.max_part <= feed);
        }
    }

    #[test]
    fn a_carriage_return_before_a_chunk_end_is_kept_when_no_newline_follows() {
        let l = split(b"a\r", 2).map(|_| ()).expect_err("unterminated");
        assert_eq!(l.status(), 400);
        let l = split(b"a\rb\n", 2).expect("ok");
        assert_eq!(texts(&l), ["a\rb"]);
        let l = split(b"\r\r\n", 1).expect("ok");
        assert_eq!(texts(&l), ["\r"]);
    }

    #[test]
    fn an_unterminated_last_line_is_refused_with_javas_message() {
        let e = split(b"{}\n{}", 64).map(|_| ()).expect_err("unterminated");
        assert_eq!(
            e.as_exception().map(|x| x.reason()),
            Some("The bulk request must be terminated by a newline [\\n]")
        );
        assert!(split(b"", 8).expect("empty").lines.is_empty());
        let mut s = Splitter::new();
        assert_eq!(s.line(), 1);
        s.feed(b"x\ny", &mut Lines::default()).expect("ok");
        assert_eq!(s.line(), 2);
    }
}
