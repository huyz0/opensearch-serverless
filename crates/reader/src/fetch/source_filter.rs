//! `_source` filtering: `FetchSourceContext.fromXContent` and
//! `XContentMapValues.filter` (server/src/main/java/org/opensearch/search/fetch/subphase/FetchSourceContext.java,
//! libs/core/src/main/java/org/opensearch/common/xcontent/support/XContentMapValues.java),
//! with the source map read and written back as `SourceLookup` and the
//! `XContentBuilder` do, at OpenSearch 3.10.0.
//!
//! A filtered source is not the stored bytes: OpenSearch reads the source
//! into hash maps and writes the filtered maps back, so every object's keys
//! come out in `java.util.HashMap` order (`source_map::java_hash_order`), numbers as
//! Java prints an `Integer`, `Long` or `Double`, and strings as Jackson
//! escapes them.

use std::io::{self, Write};

use super::source_map::{Value, read, write_value};
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser, Recorder};

/// What `_source` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFilter {
    /// `false`: no `_source` in the hits.
    pub skip: bool,
    /// The patterns a kept field matches; every field when empty.
    pub includes: Vec<String>,
    /// The patterns a dropped field matches.
    pub excludes: Vec<String>,
}

impl SourceFilter {
    /// Whether the stored source is written as stored.
    pub fn is_whole(&self) -> bool {
        !self.skip && self.includes.is_empty() && self.excludes.is_empty()
    }
}

fn parsing(p: &Parser<'_>, message: String) -> DslError {
    DslError::parsing(p.location(), message)
}

fn unknown(p: &Parser<'_>, kind: Kind, name: &str) -> DslError {
    // ALLOC: error path; one message.
    parsing(p, format!("Unknown key for a {} in [{name}].", kind.name()))
}

/// `parseSourceFieldArray` and `parseSourceArray`: strings only, a repeat
/// dropped (a `LinkedHashSet`). A bad element names `null`: Jackson has no
/// current name inside an array.
fn strings(p: &mut Parser<'_>, out: &mut Vec<String>) -> Result<(), DslError> {
    loop {
        match p.next_token() {
            Some(Kind::EndArray) | None => return Ok(()),
            Some(Kind::String) => {
                let text = p.text();
                if !out.iter().any(|s| s == text) {
                    // ALLOC: one pattern, bounded by the body's recording.
                    out.push(text.to_owned());
                }
            }
            Some(k) => return Err(unknown(p, k, "null")),
        }
    }
}

/// `FetchSourceContext.fromXContent`, the parser on the value.
pub fn parse_source(p: &mut Parser<'_>, name: &str) -> Result<SourceFilter, DslError> {
    let mut f = SourceFilter::default();
    match p.current_token() {
        Some(Kind::Bool) => f.skip = p.text() != "true",
        // ALLOC: one pattern, bounded by the body's recording.
        Some(Kind::String) => f.includes.push(p.text().to_owned()),
        Some(Kind::StartArray) => strings(p, &mut f.includes)?,
        Some(Kind::StartObject) => {
            let mut current = None;
            loop {
                match p.next_token() {
                    Some(Kind::EndObject) | None => break,
                    Some(Kind::FieldName) => current = Some(p.text()),
                    Some(k) => {
                        let Some(field) = current else {
                            return Err(parsing(
                                p,
                                // ALLOC: error path; one message.
                                format!(
                                    "Expected a field name but got a {} in [{name}].",
                                    k.name()
                                ),
                            ));
                        };
                        let into = match field {
                            "includes" | "include" => &mut f.includes,
                            "excludes" | "exclude" => &mut f.excludes,
                            _ => return Err(unknown(p, k, field)),
                        };
                        match k {
                            Kind::StartArray => {
                                into.clear();
                                strings(p, into)?;
                            }
                            Kind::String => {
                                into.clear();
                                // ALLOC: one pattern, bounded by the body's recording.
                                into.push(p.text().to_owned());
                            }
                            _ => return Err(unknown(p, k, field)),
                        }
                    }
                }
            }
        }
        k => {
            return Err(parsing(
                p,
                // ALLOC: error path; one message.
                format!(
                    "Expected one of [VALUE_BOOLEAN, VALUE_STRING, START_ARRAY, START_OBJECT] but \
                     found [{}]",
                    k.map_or("null", Kind::name)
                ),
            ));
        }
    }
    if let Some(both) = f.excludes.iter().find(|e| f.includes.contains(e)) {
        return Err(DslError::illegal(
            // ALLOC: error path; one message.
            format!("The same entry [{both}] cannot be both included and excluded in _source."),
        ));
    }
    Ok(f)
}

/// One `CharacterRunAutomaton` of `Regex.simpleMatchToAutomaton` over the
/// patterns, with `makeMatchDotsInFieldNames` (a pattern also matches
/// anything after a dot after it), run as the set of pattern positions it
/// stands for.
#[derive(Clone, Copy)]
enum Auto<'p> {
    /// `Automata.makeAnyString()`.
    All,
    /// The patterns; none is `Automata.makeEmpty()`.
    Patterns(&'p [String]),
}

/// A state: `None` is Java's `-1`, no match possible any more.
#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    /// (pattern, chars matched), `*` positions closed over.
    at: Vec<(usize, usize)>,
    /// Past a match and the dot after it: everything matches.
    tail: bool,
}

impl Auto<'_> {
    fn start(self) -> Option<State> {
        let mut s = State::default();
        if let Auto::Patterns(ps) = self {
            for i in 0..ps.len() {
                close(ps, &mut s.at, i, 0);
            }
        } else {
            s.tail = true;
        }
        Some(s)
    }

    fn accepts(self, s: &Option<State>) -> bool {
        let Some(s) = s else { return false };
        s.tail
            || match self {
                Auto::All => true,
                Auto::Patterns(ps) => {
                    s.at.iter()
                        .any(|&(i, j)| ps.get(i).is_some_and(|p| j == p.chars().count()))
                }
            }
    }

    fn step_char(self, s: Option<State>, c: char) -> Option<State> {
        let s = s?;
        if s.tail {
            return Some(s);
        }
        let Auto::Patterns(ps) = self else {
            return Some(s);
        };
        let mut next = State::default();
        for &(i, j) in &s.at {
            let Some(p) = ps.get(i) else { continue };
            match p.chars().nth(j) {
                Some('*') => close(ps, &mut next.at, i, j),
                Some(pc) if pc == c => close(ps, &mut next.at, i, j.saturating_add(1)),
                None if c == '.' => next.tail = true,
                _ => {}
            }
        }
        (next.tail || !next.at.is_empty()).then_some(next)
    }

    fn step(self, s: Option<State>, key: &str) -> Option<State> {
        key.chars().fold(s, |s, c| self.step_char(s, c))
    }
}

/// Adds position `j` of pattern `i`, and the positions after any `*` there.
fn close(ps: &[String], at: &mut Vec<(usize, usize)>, i: usize, mut j: usize) {
    loop {
        if !at.contains(&(i, j)) {
            // ALLOC: one entry a pattern position, at most the patterns' length.
            at.push((i, j));
        }
        match ps.get(i).and_then(|p| p.chars().nth(j)) {
            Some('*') => j = j.saturating_add(1),
            _ => return,
        }
    }
}

/// The filter of `XContentMapValues.filter(includes, excludes, true)`.
struct Filter<'p> {
    exclude: Auto<'p>,
}

impl Filter<'_> {
    /// `filter(Map, ...)`.
    fn object<'a>(
        &self,
        entries: Vec<(&'a str, Value<'a>)>,
        include: Auto<'_>,
        in_state: &Option<State>,
        ex_state: &Option<State>,
    ) -> Vec<(&'a str, Value<'a>)> {
        // ALLOC: one entry a kept key, at most the object's keys.
        let mut kept = Vec::new();
        for (key, value) in entries {
            let inc = include.step(in_state.clone(), key);
            if inc.is_none() {
                continue;
            }
            let exc = self.exclude.step(ex_state.clone(), key);
            if self.exclude.accepts(&exc) {
                continue;
            }
            let accepted = include.accepts(&inc);
            let (mut sub, mut sub_state) = (include, inc.clone());
            if accepted {
                if self.exclude.step_char(exc.clone(), '.').is_none() {
                    kept.push((key, value));
                    continue;
                }
                sub = Auto::All;
                sub_state = Auto::All.start();
            }
            match value {
                Value::Object(inner) => {
                    let sub_state = sub.step_char(sub_state, '.');
                    if sub_state.is_none() {
                        continue;
                    }
                    let exc = self.exclude.step_char(exc, '.');
                    let inner = self.object(inner, sub, &sub_state, &exc);
                    if accepted || !inner.is_empty() {
                        kept.push((key, Value::Object(inner)));
                    }
                }
                Value::Array(values) => {
                    let values = self.array(values, sub, &sub_state, &exc);
                    if accepted || !values.is_empty() {
                        kept.push((key, Value::Array(values)));
                    }
                }
                leaf => {
                    if accepted && !self.exclude.accepts(&exc) {
                        kept.push((key, leaf));
                    }
                }
            }
        }
        kept
    }

    /// `filter(Iterable, ...)`.
    fn array<'a>(
        &self,
        values: Vec<Value<'a>>,
        include: Auto<'_>,
        in_state: &Option<State>,
        ex_state: &Option<State>,
    ) -> Vec<Value<'a>> {
        let is_include = include.accepts(in_state);
        // ALLOC: one entry a kept value, at most the array's values.
        let mut kept = Vec::new();
        for value in values {
            match value {
                Value::Object(inner) => {
                    let inc = include.step_char(in_state.clone(), '.');
                    let exc = self.exclude.step_char(ex_state.clone(), '.');
                    let inner = self.object(inner, include, &inc, &exc);
                    if !inner.is_empty() {
                        kept.push(Value::Object(inner));
                    }
                }
                Value::Array(inner) => {
                    let inner = self.array(inner, include, in_state, ex_state);
                    if !inner.is_empty() {
                        kept.push(Value::Array(inner));
                    }
                }
                leaf if is_include => kept.push(leaf),
                _ => {}
            }
        }
        kept
    }
}

/// `createSetBasedFilter`: neither list has a `*` or a `.`, so only the
/// top-level keys are matched, each by its part before a first dot.
fn set_filter<'a>(
    f: &SourceFilter,
    entries: Vec<(&'a str, Value<'a>)>,
) -> Vec<(&'a str, Value<'a>)> {
    entries
        .into_iter()
        .filter(|(key, _)| {
            let k = match key.find('.') {
                Some(dot) if dot > 0 => key.get(..dot).unwrap_or(key),
                _ => key,
            };
            (f.includes.is_empty() || f.includes.iter().any(|i| i == k))
                && !f.excludes.iter().any(|e| e == k)
        })
        // ALLOC: one entry a kept key, at most the source's top-level keys.
        .collect()
}

/// The most bytes filtering a stored source of `len` bytes holds at once:
/// its recorded tokens and text, the value tree (at most one value a byte
/// pair), and the orderings. A bound, not an estimate.
pub fn scratch_bytes(len: usize) -> usize {
    len.saturating_mul(SCRATCH_PER_BYTE).saturating_add(256)
}

/// The bytes of scratch a stored byte may take when filtered.
const SCRATCH_PER_BYTE: usize = 64;

/// Writes `source` (a JSON object, as stored) filtered by `f`.
///
/// # Errors
/// A source that is not a JSON object, or a write failure.
pub fn write_filtered(out: &mut dyn Write, f: &SourceFilter, source: &[u8]) -> io::Result<()> {
    // ALLOC: the source's tokens and text, bounded by the stored source,
    // which the caller reserved.
    let mut rec = Recorder::new(scratch_bytes(source.len()));
    let mut tokenizer = crate::dsl::source::tokenizer();
    tokenizer
        .feed(source, &mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    tokenizer
        .finish(&mut rec)
        .map_err(|e| io::Error::other(e.into_failure().reason))?;
    let mut p = rec.parser();
    let Some(Kind::StartObject) = p.next_token() else {
        return Err(io::Error::other("a _source that is not a JSON object"));
    };
    let Value::Object(entries) = read(&mut p, Kind::StartObject) else {
        return Err(io::Error::other("a _source that is not a JSON object"));
    };
    let wild = |ps: &[String]| ps.iter().any(|s| s.contains('*') || s.contains('.'));
    let kept = if !wild(&f.includes) && !wild(&f.excludes) {
        set_filter(f, entries)
    } else {
        let include = if f.includes.is_empty() {
            Auto::All
        } else {
            Auto::Patterns(&f.includes)
        };
        let filter = Filter {
            exclude: Auto::Patterns(&f.excludes),
        };
        filter.object(entries, include, &include.start(), &filter.exclude.start())
    };
    // ALLOC: one entry a key of the widest object written.
    let mut order = Vec::new();
    write_value(out, &Value::Object(kept), &mut order)
}

#[cfg(test)]
#[path = "source_filter_tests.rs"]
mod tests;
