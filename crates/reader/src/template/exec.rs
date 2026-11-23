//! Running compiled codes over the params: `IterableCode`, `ValueCode`,
//! `NotIterableCode`, the custom functions and `ReflectionObjectHandler.find`.

use std::borrow::Cow;

use super::TemplateError;
use super::members;
use super::parse::{self, Func, Kind, Node};
use super::val::{Val, json_escape, url_encode};

/// How deep sections and function fragments may run into each other.
const MAX_DEPTH: usize = 512;

/// A value a scope or a lookup holds: a node of the params, or the size of
/// an array, which `CollectionMap` answers for `size`.
#[derive(Clone, Copy)]
enum S<'a> {
    V(&'a Val),
    Size(usize),
}

impl<'a> S<'a> {
    fn val(self) -> Cow<'a, Val> {
        match self {
            S::V(v) => Cow::Borrowed(v),
            S::Size(n) => Cow::Owned(Val::Int(i64::try_from(n).unwrap_or(i64::MAX))),
        }
    }
}

/// What a member lookup found.
enum Member<'a> {
    Found(S<'a>),
    Missing,
}

pub(super) struct Exec<'a> {
    scopes: Vec<S<'a>>,
    out: String,
    limit: usize,
    depth: usize,
}

fn runtime(message: impl Into<String>) -> TemplateError {
    TemplateError::Runtime(message.into())
}

/// `new MustacheException(message, tc)`: the message and where the code is.
fn at(message: &str, line: i64) -> TemplateError {
    // ALLOC: the template, its params and its output, which the request limits bound.
    TemplateError::Runtime(format!("{message} @[query-template:{line}]"))
}

/// What the code's `get` throws when the lookup does: a value code names
/// itself, the others carry the cause's message.
fn lookup_error(e: TemplateError, node: &Node) -> TemplateError {
    match (e, &node.kind) {
        (TemplateError::Runtime(_), Kind::Value { name, .. }) => {
            // ALLOC: the template, its params and its output, which the request limits bound.
            at(&format!("Failed to get value for {name}"), node.line)
        }
        (TemplateError::Runtime(inner), _) => at(&inner, node.line),
        (other, _) => other,
    }
}

/// `Integer.parseInt(key)`, `None` when it throws `NumberFormatException`.
fn parse_int(key: &str) -> Option<i32> {
    let digits = key.strip_prefix(['+', '-']).unwrap_or(key);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.strip_prefix('+').unwrap_or(key).parse().ok()
}

impl<'a> Exec<'a> {
    pub(super) fn new(root: &'a Val, limit: usize) -> Self {
        Self {
            // ALLOC: the scope stack, one entry a nested section, at most MAX_DEPTH.
            scopes: vec![S::V(root)],
            out: String::new(),
            limit,
            depth: 0,
        }
    }

    pub(super) fn finish(self) -> String {
        self.out
    }

    fn push(&mut self, text: &str) -> Result<(), TemplateError> {
        if self.out.len().saturating_add(text.len()) > self.limit {
            return Err(TemplateError::TooLarge(self.limit));
        }
        self.out.push_str(text);
        Ok(())
    }

    pub(super) fn run(&mut self, codes: &[Node]) -> Result<(), TemplateError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > MAX_DEPTH {
            // ALLOC: the template, its params and its output, which the request limits bound.
            return Err(TemplateError::Unsupported(format!(
                "templates nested more than [{MAX_DEPTH}] deep are not supported by this node yet"
            )));
        }
        for node in codes {
            self.node(node)?;
        }
        self.depth = self.depth.saturating_sub(1);
        Ok(())
    }

    /// Runs `f` with the output swapped for an empty buffer, and returns
    /// what it wrote: the `StringWriter` a function captures into.
    fn capture(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), TemplateError>,
    ) -> Result<String, TemplateError> {
        let outer = std::mem::take(&mut self.out);
        let result = f(self);
        let captured = std::mem::replace(&mut self.out, outer);
        result.map(|()| captured)
    }

    fn node(&mut self, node: &Node) -> Result<(), TemplateError> {
        match &node.kind {
            Kind::Write
            | Kind::Eof
            | Kind::Comment { .. }
            | Kind::Partial { .. }
            | Kind::Extend { .. } => {}
            Kind::Value { name, encoded } => {
                if let Some(v) = self.get(name).map_err(|e| lookup_error(e, node))? {
                    let text = v.val().stringify();
                    if *encoded {
                        // ALLOC: the template, its params and its output, which the request limits bound.
                        let mut escaped = String::new();
                        json_escape(&text, &mut escaped);
                        self.push(&escaped)?;
                    } else {
                        self.push(&text)?;
                    }
                }
            }
            Kind::Section {
                name,
                body,
                inverted,
            } => {
                let resolved = self.get(name).map_err(|e| lookup_error(e, node))?;
                if *inverted {
                    self.falsey(resolved, body)?;
                } else {
                    self.iterate(resolved, body)?;
                }
            }
            Kind::Function { func, name, body } => {
                let resolved = self.get(name).map_err(|e| lookup_error(e, node))?;
                // ALLOC: the template, its params and its output, which the request limits bound.
                let mut text = String::new();
                parse::identity(body, &mut text);
                let fragment = parse::compile(&text, &node.sm, &node.em)?;
                let captured = self.capture(|exec| exec.run(&fragment))?;
                let resolved = resolved.map(S::val);
                let applied = match func {
                    Func::ToJson => resolved.map(|v| match &*v {
                        json @ (Val::List(_) | Val::Map(_)) => {
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            let mut text = String::new();
                            json.write_json(&mut text);
                            text
                        }
                        // `oh.stringify(resolved)`: not handled as JSON.
                        scalar => scalar.stringify(),
                    }),
                    Func::Join(delimiter, _) => Some(match resolved.as_deref() {
                        Some(Val::List(items)) => {
                            // ALLOC: the template, its params and its output, which the request limits bound.
                            let mut joined = String::new();
                            for (i, item) in items.iter().enumerate() {
                                if i > 0 {
                                    joined.push_str(delimiter);
                                }
                                // `oh.stringify(null)` throws a `NullPointerException`.
                                if matches!(item, Val::Null) {
                                    return Err(at("Function failure", node.line));
                                }
                                joined.push_str(&item.element_string());
                            }
                            joined
                        }
                        _ => captured,
                    }),
                };
                if let Some(text) = applied {
                    self.push(&text)?;
                }
            }
            Kind::Url { body } => {
                for code in body {
                    let captured = self.capture(|exec| exec.node(code))?;
                    // ALLOC: the template, its params and its output, which the request limits bound.
                    let mut encoded = String::new();
                    url_encode(&captured, &mut encoded);
                    self.push(&encoded)?;
                }
            }
        }
        self.push(&node.appended)
    }

    /// `DefaultCode.get`: `.` is the innermost scope, anything else a lookup.
    /// A JSON null is no value.
    fn get(&self, name: &str) -> Result<Option<S<'a>>, TemplateError> {
        let found = if name == "." {
            self.scopes.last().copied()
        } else {
            self.find(name)?
        };
        Ok(found.filter(|s| !matches!(s, S::V(Val::Null))))
    }

    /// `ReflectionObjectHandler.find`: the name in the innermost scope that
    /// has it, dotted names walked one step at a time; a step that is missing
    /// moves out a scope, and one that is null ends the search.
    fn find(&self, name: &str) -> Result<Option<S<'a>>, TemplateError> {
        for scope in self.scopes.iter().rev().copied() {
            if let Member::Found(v) = member(scope, name)? {
                return Ok(Some(v));
            }
            let mut current = scope;
            let mut rest = name;
            let mut walked = false;
            let mut ended_null = false;
            let mut next_scope = false;
            while let Some((lookup, tail)) = rest.split_once('.') {
                rest = tail;
                match member(current, lookup)? {
                    Member::Found(v) => {
                        walked = true;
                        current = v;
                        if matches!(v, S::V(Val::Null)) {
                            ended_null = true;
                            break;
                        }
                    }
                    Member::Missing => {
                        next_scope = true;
                        break;
                    }
                }
            }
            if next_scope {
                continue;
            }
            if walked {
                if ended_null {
                    return Ok(None);
                }
                return Ok(match member(current, rest)? {
                    Member::Found(v) => Some(v),
                    Member::Missing => None,
                });
            }
        }
        Ok(None)
    }

    /// `AbstractObjectHandler.iterate`.
    fn iterate(&mut self, resolved: Option<S<'a>>, body: &[Node]) -> Result<(), TemplateError> {
        let Some(s) = resolved else {
            return Ok(());
        };
        match s {
            S::V(Val::Bool(false)) => Ok(()),
            S::V(Val::Str(text)) if text.is_empty() => Ok(()),
            S::V(Val::List(items)) => {
                for item in items {
                    self.next(Some(S::V(item)), body)?;
                }
                Ok(())
            }
            other => self.next(Some(other), body),
        }
    }

    /// `AbstractObjectHandler.falsey`: the body runs for a value that is
    /// not true, not a non-empty string or array, and not another object.
    fn falsey(&mut self, resolved: Option<S<'a>>, body: &[Node]) -> Result<(), TemplateError> {
        let renders = match resolved {
            None => true,
            Some(S::V(Val::Bool(b))) => !*b,
            Some(S::V(Val::Str(text))) => text.is_empty(),
            Some(S::V(Val::List(items))) => items.is_empty(),
            Some(_) => false,
        };
        if renders {
            self.run(body)?;
        }
        Ok(())
    }

    /// `IterableCode.next`: a value that is not null is a scope for the body.
    fn next(&mut self, item: Option<S<'a>>, body: &[Node]) -> Result<(), TemplateError> {
        let pushed = match item {
            Some(S::V(Val::Null)) | None => false,
            Some(s) => {
                self.scopes.push(s);
                true
            }
        };
        let result = self.run(body);
        if pushed {
            self.scopes.pop();
        }
        result
    }
}

/// `findWrapper`: what `scope` holds under `key`.
fn member<'a>(scope: S<'a>, key: &str) -> Result<Member<'a>, TemplateError> {
    let v = match scope {
        S::Size(n) => return scalar(&Val::Int(i64::try_from(n).unwrap_or(i64::MAX)), key),
        S::V(v) => v,
    };
    match v {
        Val::Map(entries) => Ok(entries
            .iter()
            .find(|(k, _)| k == key)
            .map_or(Member::Missing, |(_, v)| Member::Found(S::V(v)))),
        Val::List(items) => {
            if key == "size" {
                return Ok(Member::Found(S::Size(items.len())));
            }
            let Some(index) = parse_int(key) else {
                return Ok(Member::Missing);
            };
            match usize::try_from(index).ok().and_then(|i| items.get(i)) {
                Some(Val::Null) => Ok(Member::Missing),
                Some(item) => Ok(Member::Found(S::V(item))),
                None if index < 0 => Err(runtime("position >= 0")),
                // ALLOC: the template, its params and its output, which the request limits bound.
                None => Err(runtime(index.to_string())),
            }
        }
        Val::Null => Ok(Member::Missing),
        scalar_value => scalar(scalar_value, key),
    }
}

/// A scalar scope has the members of its Java class, which this node does
/// not read.
fn scalar<'a>(value: &Val, key: &str) -> Result<Member<'a>, TemplateError> {
    if members::resolves(value, key) {
        // ALLOC: the template, its params and its output, which the request limits bound.
        return Err(TemplateError::Unsupported(format!(
            "reading [{key}] of a string, number or boolean scope is not supported by this node yet"
        )));
    }
    Ok(Member::Missing)
}
