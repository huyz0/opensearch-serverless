//! Tokens of the plugin's SQL dialect, bounded in bytes, count and identifier length.

use crate::{Error, Limits};

/// One token and where it starts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) pos: usize,
}

/// What a token is.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// A bare word: a keyword or an unquoted name, case kept.
    Word(String),
    /// A name in backticks or double quotes.
    Quoted(String),
    /// A single-quoted string.
    Str(String),
    /// An integer.
    Int(i64),
    /// A decimal.
    Float(f64),
    /// An operator or punctuation.
    Sym(&'static str),
}

const SYMS: [&str; 19] = [
    "<>", "!=", "<=", ">=", "=", "<", ">", "(", ")", ",", ".", "*", "+", "-", "/", "%", "[", "]",
    "|",
];

fn word_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'@' || c == b'#'
}

fn word_part(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'@' | b'*' | b'^')
}

/// Splits a statement into tokens.
///
/// # Errors
///
/// A bound passed or a character the dialect has no token for.
pub(crate) fn lex(sql: &str, limits: &Limits) -> Result<Vec<Token>, Error> {
    if sql.len() > limits.max_statement_bytes {
        return Err(Error::bound("statement bytes", limits.max_statement_bytes));
    }
    let b = sql.as_bytes();
    // ALLOC: at most `max_tokens` tokens, checked on every push.
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i = i.saturating_add(1);
            continue;
        }
        if c == b'-' && b.get(i.saturating_add(1)) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i = i.saturating_add(1);
            }
            continue;
        }
        if c == b'/' && b.get(i.saturating_add(1)) == Some(&b'*') {
            i = i.saturating_add(2);
            loop {
                if i >= b.len() {
                    return Err(Error::syntax("/*", start));
                }
                if b[i] == b'*' && b.get(i.saturating_add(1)) == Some(&b'/') {
                    i = i.saturating_add(2);
                    break;
                }
                i = i.saturating_add(1);
            }
            continue;
        }
        let kind = if c == b'\'' {
            let (text, next) = quoted(sql, i, b'\'')?;
            i = next;
            Kind::Str(text)
        } else if c == b'`' || c == b'"' {
            let (text, next) = quoted(sql, i, c)?;
            i = next;
            if text.len() > limits.max_ident {
                return Err(Error::bound("identifier length", limits.max_ident));
            }
            Kind::Quoted(text)
        } else if c.is_ascii_digit() {
            let (kind, next) = number(sql, i)?;
            i = next;
            kind
        } else if word_start(c)
            || (c == b'.'
                && b.get(i.saturating_add(1)).is_some_and(|d| word_start(*d))
                && prev_is_not_name(&out))
        {
            i = i.saturating_add(1);
            while i < b.len() && word_part(b[i]) {
                i = i.saturating_add(1);
            }
            let word = &sql[start..i];
            if word.len() > limits.max_ident {
                return Err(Error::bound("identifier length", limits.max_ident));
            }
            // ALLOC: one word, bounded by `max_ident`.
            Kind::Word(word.to_owned())
        } else if let Some(sym) = SYMS.iter().find(|s| sql[i..].starts_with(**s)) {
            i = i.saturating_add(sym.len());
            Kind::Sym(sym)
        } else {
            let ch = sql[i..].chars().next().unwrap_or('?');
            return Err(Error::syntax(&ch.to_string(), i));
        };
        if out.len() >= limits.max_tokens {
            return Err(Error::bound("tokens", limits.max_tokens));
        }
        out.push(Token { kind, pos: start });
    }
    Ok(out)
}

/// Whether a `.` followed by a letter starts a name (`.kibana`) rather than a path step (`a.b`).
fn prev_is_not_name(out: &[Token]) -> bool {
    !matches!(
        out.last().map(|t| &t.kind),
        Some(Kind::Word(_) | Kind::Quoted(_))
    )
}

/// Reads a quoted run; a doubled quote is one quote.
fn quoted(sql: &str, open: usize, q: u8) -> Result<(String, usize), Error> {
    let b = sql.as_bytes();
    let mut i = open.saturating_add(1);
    let mut from = i;
    // ALLOC: bounded by the statement's byte limit.
    let mut text = String::new();
    loop {
        match b.get(i) {
            None => return Err(Error::syntax(&sql[open..open.saturating_add(1)], open)),
            Some(&c) if c == q => {
                if b.get(i.saturating_add(1)) == Some(&q) {
                    text.push_str(&sql[from..=i]);
                    i = i.saturating_add(2);
                    from = i;
                } else {
                    text.push_str(&sql[from..i]);
                    return Ok((text, i.saturating_add(1)));
                }
            }
            Some(_) => i = i.saturating_add(1),
        }
    }
}

fn number(sql: &str, start: usize) -> Result<(Kind, usize), Error> {
    let b = sql.as_bytes();
    let mut i = start;
    while i < b.len() && b[i].is_ascii_digit() {
        i = i.saturating_add(1);
    }
    let mut float = false;
    if b.get(i) == Some(&b'.') && b.get(i.saturating_add(1)).is_some_and(u8::is_ascii_digit) {
        float = true;
        i = i.saturating_add(1);
        while i < b.len() && b[i].is_ascii_digit() {
            i = i.saturating_add(1);
        }
    }
    let text = &sql[start..i];
    if float {
        text.parse::<f64>()
            .map(|f| (Kind::Float(f), i))
            .map_err(|_| Error::syntax(text, start))
    } else {
        text.parse::<i64>()
            .map(|n| (Kind::Int(n), i))
            .map_err(|_| Error::syntax(text, start))
    }
}
