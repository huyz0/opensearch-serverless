//! A small strict JSON reader for a descriptor: bounded depth, no duplicate
//! keys, integers kept apart from other numbers, nothing after the value.

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A non-negative integer that fits `u64`.
    Int(u64),
    /// Any other number (negative, fractional, exponent, too large).
    Number,
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Value>),
    /// An object, members in source order, keys unique.
    Obj(Vec<(String, Value)>),
}

/// Why a document did not parse.
#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    /// Not valid JSON, nested too deep, or trailing text.
    Syntax,
    /// An object repeats this key.
    Duplicate(String),
}

/// The deepest level a value may sit at, the root being 0: the root object and
/// seven containers inside it, whose scalars sit at 8.
const MAX_DEPTH: usize = 8;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Parses `text` as one JSON value.
pub fn parse(text: &str) -> Result<Value, Fail> {
    let mut r = Reader {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = r.value(0)?;
    r.skip_space();
    if r.pos < r.bytes.len() {
        return Err(Fail::Syntax);
    }
    Ok(value)
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos = self.pos.saturating_add(1);
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    fn eat(&mut self, byte: u8) -> Result<(), Fail> {
        if self.peek() == Some(byte) {
            self.bump();
            Ok(())
        } else {
            Err(Fail::Syntax)
        }
    }

    fn word(&mut self, word: &[u8], value: Value) -> Result<Value, Fail> {
        let end = self.pos.saturating_add(word.len());
        if self.bytes.get(self.pos..end) == Some(word) {
            self.pos = end;
            Ok(value)
        } else {
            Err(Fail::Syntax)
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Fail> {
        if depth > MAX_DEPTH {
            return Err(Fail::Syntax);
        }
        self.skip_space();
        match self.peek().ok_or(Fail::Syntax)? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Value::Str),
            b't' => self.word(b"true", Value::Bool(true)),
            b'f' => self.word(b"false", Value::Bool(false)),
            b'n' => self.word(b"null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(Fail::Syntax),
        }
    }

    fn number(&mut self) -> Result<Value, Fail> {
        let start = self.pos;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.bump();
        }
        let digits_from = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.bump();
        }
        let digits = self.bytes.get(digits_from..self.pos).unwrap_or_default();
        if digits.is_empty() || (digits.len() > 1 && digits.first() == Some(&b'0')) {
            return Err(Fail::Syntax);
        }
        let mut plain = !negative;
        if self.peek() == Some(b'.') {
            plain = false;
            self.bump();
            let from = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.pos == from {
                return Err(Fail::Syntax);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            plain = false;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            let from = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
            if self.pos == from {
                return Err(Fail::Syntax);
            }
        }
        if !plain {
            return Ok(Value::Number);
        }
        let mut n: u64 = 0;
        for d in self.bytes.get(start..self.pos).unwrap_or_default() {
            let digit = u64::from(d.saturating_sub(b'0'));
            match n.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                Some(v) => n = v,
                None => return Ok(Value::Number),
            }
        }
        Ok(Value::Int(n))
    }

    fn hex4(&mut self) -> Result<u32, Fail> {
        let mut v = 0u32;
        for _ in 0..4 {
            let d = self
                .peek()
                .and_then(|b| (b as char).to_digit(16))
                .ok_or(Fail::Syntax)?;
            v = v
                .checked_mul(16)
                .and_then(|x| x.checked_add(d))
                .ok_or(Fail::Syntax)?;
            self.bump();
        }
        Ok(v)
    }

    fn string(&mut self) -> Result<String, Fail> {
        self.eat(b'"')?;
        // ALLOC: a descriptor is at most MAX_DESCRIPTOR_BYTES, checked before parsing.
        let mut out = String::new();
        loop {
            let b = self.peek().ok_or(Fail::Syntax)?;
            match b {
                b'"' => {
                    self.bump();
                    return Ok(out);
                }
                b'\\' => {
                    self.bump();
                    let e = self.peek().ok_or(Fail::Syntax)?;
                    self.bump();
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let c = if (0xd800..0xdc00).contains(&first) {
                                self.eat(b'\\')?;
                                self.eat(b'u')?;
                                let second = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&second) {
                                    return Err(Fail::Syntax);
                                }
                                // ARITH: both halves are surrogates, so the result is below 0x110000.
                                #[allow(clippy::arithmetic_side_effects)]
                                let joined = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                                joined
                            } else {
                                first
                            };
                            out.push(char::from_u32(c).ok_or(Fail::Syntax)?);
                        }
                        _ => return Err(Fail::Syntax),
                    }
                }
                0..=0x1f => return Err(Fail::Syntax),
                _ => {
                    // The input is `&str`, so a multi-byte character is whole; copy it by its length.
                    let rest = self.bytes.get(self.pos..).unwrap_or_default();
                    let text = std::str::from_utf8(rest.get(..char_len(b)).unwrap_or_default())
                        .map_err(|_| Fail::Syntax)?;
                    out.push_str(text);
                    self.pos = self.pos.saturating_add(char_len(b));
                }
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Fail> {
        self.eat(b'[')?;
        // ALLOC: bounded by the descriptor's size.
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.bump();
            return Ok(Value::Arr(items));
        }
        loop {
            items.push(self.value(depth.saturating_add(1))?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b']') => {
                    self.bump();
                    return Ok(Value::Arr(items));
                }
                _ => return Err(Fail::Syntax),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Fail> {
        self.eat(b'{')?;
        // ALLOC: bounded by the descriptor's size.
        let mut members: Vec<(String, Value)> = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.bump();
            return Ok(Value::Obj(members));
        }
        loop {
            self.skip_space();
            let key = self.string()?;
            self.skip_space();
            self.eat(b':')?;
            let value = self.value(depth.saturating_add(1))?;
            if members.iter().any(|(k, _)| *k == key) {
                return Err(Fail::Duplicate(key));
            }
            members.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.bump(),
                Some(b'}') => {
                    self.bump();
                    return Ok(Value::Obj(members));
                }
                _ => return Err(Fail::Syntax),
            }
        }
    }
}

/// The length of the UTF-8 character that starts with `lead`.
fn char_len(lead: u8) -> usize {
    match lead {
        0..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}
