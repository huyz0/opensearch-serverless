//! A document's `_source` with each indexed vector's value replaced by the
//! number `1`, as the k-NN plugin's derived source stores it
//! (`KNN10010DerivedSourceStoredFieldsFormat`): only the bytes of the
//! vectors' values change.

use crate::mapping::MappingError;

/// `source` with the value of each of `fields` (full dotted paths) replaced
/// by `1`.
///
/// # Errors
///
/// [`MappingError::unsupported`] when a field cannot be placed by its path
/// among the source's object keys (an escaped key, a dotted key, a value
/// inside an array) or the source is not a JSON object: the caller keeps the
/// document on a Java writer.
pub fn mask_vectors(source: &[u8], fields: &[&str]) -> Result<Vec<u8>, MappingError> {
    let mut scan = Scan {
        src: source,
        pos: 0,
        fields,
        found: 0,
        // ALLOC: one buffer a document with a vector, its source's length.
        out: Vec::with_capacity(source.len()),
        copied: 0,
    };
    scan.skip_ws();
    scan.object(&mut String::new())?;
    scan.skip_ws();
    if scan.pos != source.len() {
        return Err(cannot_place("bytes follow the source's object"));
    }
    if scan.found < fields.len() {
        return Err(cannot_place(
            "a vector field is not at its path in the source",
        ));
    }
    scan.out
        .extend_from_slice(source.get(scan.copied..).unwrap_or(&[]));
    Ok(scan.out)
}

fn cannot_place(why: &str) -> MappingError {
    // ALLOC: error path; one message.
    MappingError::unsupported(&format!("derived source of a knn_vector: {why}"))
}

struct Scan<'a> {
    src: &'a [u8],
    pos: usize,
    fields: &'a [&'a str],
    found: usize,
    out: Vec<u8>,
    /// Source bytes before this are in `out` already.
    copied: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos = self.pos.saturating_add(1);
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), MappingError> {
        self.skip_ws();
        if self.peek() == Some(byte) {
            self.pos = self.pos.saturating_add(1);
            Ok(())
        } else {
            Err(cannot_place(
                "the source is not the JSON object it was parsed as",
            ))
        }
    }

    /// An object member's key, which must hold no escape.
    fn key(&mut self) -> Result<&[u8], MappingError> {
        self.expect(b'"')?;
        let start = self.pos;
        loop {
            match self.peek() {
                Some(b'"') => break,
                Some(b'\\') | None => return Err(cannot_place("an escaped key")),
                Some(_) => self.pos = self.pos.saturating_add(1),
            }
        }
        let key = self.src.get(start..self.pos).unwrap_or(&[]);
        self.pos = self.pos.saturating_add(1);
        Ok(key)
    }

    /// The object at `pos`, its keys under `prefix` (empty at the root).
    fn object(&mut self, prefix: &mut String) -> Result<(), MappingError> {
        self.expect(b'{')?;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos = self.pos.saturating_add(1);
            return Ok(());
        }
        loop {
            let key = std::str::from_utf8(self.key()?)
                .map_err(|_| cannot_place("a key that is not UTF-8"))?
                .to_owned();
            let keep = prefix.len();
            if !prefix.is_empty() {
                prefix.push('.');
            }
            prefix.push_str(&key);
            if key.contains('.')
                && self.fields.iter().any(|f| {
                    *f == prefix.as_str()
                        || f.strip_prefix(prefix.as_str())
                            .is_some_and(|r| r.starts_with('.'))
                })
            {
                return Err(cannot_place("a dotted key"));
            }
            self.expect(b':')?;
            self.skip_ws();
            let start = self.pos;
            if self.fields.contains(&prefix.as_str()) {
                self.skip_value()?;
                self.out
                    .extend_from_slice(self.src.get(self.copied..start).unwrap_or(&[]));
                self.out.push(b'1');
                self.copied = self.pos;
                self.found = self.found.saturating_add(1);
            } else if self.peek() == Some(b'{') {
                self.object(prefix)?;
            } else {
                self.skip_value()?;
            }
            prefix.truncate(keep);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos = self.pos.saturating_add(1),
                Some(b'}') => {
                    self.pos = self.pos.saturating_add(1);
                    return Ok(());
                }
                _ => {
                    return Err(cannot_place(
                        "the source is not the JSON object it was parsed as",
                    ));
                }
            }
        }
    }

    /// Moves past the value at `pos`.
    fn skip_value(&mut self) -> Result<(), MappingError> {
        let mut depth = 0_usize;
        let mut in_string = false;
        let start = self.pos;
        while let Some(b) = self.peek() {
            if in_string {
                match b {
                    b'\\' => self.pos = self.pos.saturating_add(1),
                    b'"' => {
                        in_string = false;
                        if depth == 0 {
                            self.pos = self.pos.saturating_add(1);
                            return Ok(());
                        }
                    }
                    _ => {}
                }
            } else {
                match b {
                    b'"' => in_string = true,
                    b'{' | b'[' => depth = depth.saturating_add(1),
                    b'}' | b']' if depth == 0 => break,
                    b'}' | b']' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            self.pos = self.pos.saturating_add(1);
                            return Ok(());
                        }
                    }
                    b',' if depth == 0 => break,
                    b if b.is_ascii_whitespace() && depth == 0 => break,
                    _ => {}
                }
            }
            self.pos = self.pos.saturating_add(1);
        }
        // A value cut off by the end of the source is caught by the caller, which
        // finds no `,` or `}` after it; an empty value is caught here.
        if self.pos == start {
            return Err(cannot_place(
                "the source is not the JSON object it was parsed as",
            ));
        }
        Ok(())
    }
}
