//! `simple_query_string` as a Lucene query: Lucene's `SimpleQueryParser`
//! (the grammar) with OpenSearch's `SimpleQueryStringQueryParser` building
//! each leaf, and `SimpleQueryStringBuilder.doToQuery`, at OpenSearch
//! 3.10.0. The grammar follows the engine's `lucene-queryparser` port
//! statement for statement; what it builds at each leaf is OpenSearch's:
//! `most_fields` of the resolved fields for a word, `phrase` for a phrase,
//! and one query a field combined by `dis_max` for a prefix or a fuzzy term.

// ARITH: every sum here steps an index through the query text or counts its
// characters, so none exceeds the text (bounded by the body, far below
// `usize::MAX` and `i32::MAX`); Java adds them unchecked too.
#![allow(clippy::arithmetic_side_effects)]

use lucene_search::{BooleanQuery, Clause};

use crate::dsl::bool_query::MAX_CLAUSES;
use crate::dsl::queries::full_text_build::resolve;
use crate::dsl::queries::simple_string::SimpleQueryString;
use crate::dsl::query::Operator;
use crate::failure::Failure;
use crate::lucene_query::{
    QueryContext, apply_min_should_match, match_all, match_none, unsupported,
};

const AND: i32 = 1;
const NOT: i32 = 1 << 1;
const OR: i32 = 1 << 2;
const PREFIX: i32 = 1 << 3;
const PHRASE: i32 = 1 << 4;
const PRECEDENCE: i32 = 1 << 5;
const ESCAPE: i32 = 1 << 6;
const WHITESPACE: i32 = 1 << 7;
const FUZZY: i32 = 1 << 8;
const NEAR: i32 = 1 << 9;

/// `LevenshteinAutomata.MAXIMUM_SUPPORTED_DISTANCE`.
const MAXIMUM_SUPPORTED_DISTANCE: i32 = 2;

/// The deepest `(` nesting a parse accepts: each level recurses, and a Rust
/// stack overflow aborts the process where Java's is caught. OpenSearch
/// refuses beyond 1,000; this node refuses beyond this.
const MAX_GROUP_DEPTH: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Occur {
    Must,
    Should,
    MustNot,
}

/// A query being built: a boolean's clauses are kept in the order they
/// were added, as `BooleanQuery.Builder` keeps them.
pub(super) enum Node {
    Leaf(Clause),
    MatchAll,
    Bool(Vec<(Occur, Node)>),
}

struct State {
    index: usize,
    length: usize,
    current: Option<Occur>,
    previous: Option<Occur>,
    not: i32,
    top: Option<Node>,
}

impl State {
    fn new(index: usize, length: usize) -> Self {
        State {
            index,
            length,
            current: None,
            previous: None,
            not: 0,
            top: None,
        }
    }
}

pub(super) struct Run<'a, 'c> {
    pub(super) cx: &'a QueryContext<'c>,
    pub(super) s: &'a SimpleQueryString,
    /// The resolved fields, each with its boost.
    pub(super) fields: Vec<(&'a str, f32)>,
    pub(super) lenient: bool,
    pub(super) operator: Occur,
    pub(super) data: Vec<u16>,
    pub(super) buffer: Vec<u16>,
    pub(super) depth: usize,
    /// Loop turns taken so far, against [`Self::budget`].
    steps: usize,
    /// The most loop turns a parse of this text can need.
    budget: usize,
}

fn unit(c: char) -> u16 {
    c as u16
}

fn to_clause(node: Node, cx: &QueryContext<'_>) -> Clause {
    match node {
        Node::Leaf(c) => c,
        Node::MatchAll => match_all(cx),
        Node::Bool(parts) => {
            let mut q = BooleanQuery::default();
            for (occur, n) in parts {
                let c = to_clause(n, cx);
                match occur {
                    Occur::Must => q.must.push(c),
                    Occur::Should => q.should.push(c),
                    Occur::MustNot => q.must_not.push(c),
                }
            }
            // ALLOC: one box a boolean of the query string.
            Clause::Boolean(Box::new(q))
        }
    }
}

/// `SimpleQueryStringBuilder.doToQuery`.
pub fn simple_query_string(
    cx: &QueryContext<'_>,
    s: &SimpleQueryString,
) -> Result<Clause, Failure> {
    // ALLOC: one entry a listed field, at most MAX_LISTED_FIELDS.
    let listed: Vec<(String, f32)> = if s.fields.is_empty() {
        vec![("*".to_owned(), 1.0)]
    } else {
        s.fields.clone()
    };
    let all_fields = listed.iter().any(|(f, _)| f == "*");
    let fields = resolve(cx, &listed)?;
    let mut run = Run {
        cx,
        s,
        fields,
        lenient: s.lenient.unwrap_or(all_fields),
        operator: if s.operator == Operator::And {
            Occur::Must
        } else {
            Occur::Should
        },
        // ALLOC: the query's UTF-16 units, bounded by the body's recording.
        data: s.query.encode_utf16().collect(),
        buffer: Vec::new(),
        depth: 0,
        steps: 0,
        budget: s.query.len().saturating_mul(1000).saturating_add(1000),
    };
    let query = run.parse()?;
    Ok(match query {
        Clause::Boolean(b) => {
            // ALLOC: one box a boolean with its minimum should match.
            Clause::Boolean(Box::new(apply_min_should_match(
                *b,
                s.minimum_should_match.as_deref(),
            )?))
        }
        other => other,
    })
}

impl Run<'_, '_> {
    fn has(&self, flag: i32) -> bool {
        self.s.flags & flag != 0
    }

    /// One turn of a parsing loop. Each turn moves on in the text, so a
    /// parse needs far fewer than the budget; running out means a loop
    /// stopped moving, which fails the query rather than hanging the node.
    fn tick(&mut self) -> Result<(), Failure> {
        self.steps += 1;
        if self.steps > self.budget {
            return Err(Failure::plain(
                "illegal_state_exception",
                "the simple_query_string parser made no progress",
            ));
        }
        Ok(())
    }

    fn at(&self, i: usize) -> u16 {
        self.data.get(i).copied().unwrap_or(0)
    }

    /// `SimpleQueryParser.parse`.
    fn parse(&mut self) -> Result<Clause, Failure> {
        let trimmed = self.s.query.trim_matches(|c: char| c <= ' ');
        if trimmed == "*" {
            return Ok(match_all(self.cx));
        }
        let length = self.data.len();
        // ALLOC: the scratch of one token, at most the query's length.
        self.buffer = vec![0; length];
        let mut state = State::new(0, length);
        self.parse_sub_query(&mut state)?;
        Ok(match state.top {
            Some(top) => to_clause(top, self.cx),
            None => match_none(),
        })
    }

    fn parse_sub_query(&mut self, state: &mut State) -> Result<(), Failure> {
        while state.index < state.length {
            self.tick()?;
            let c = self.at(state.index);
            if c == unit('(') && self.has(PRECEDENCE) {
                self.consume_sub_query(state)?;
            } else if c == unit(')') && self.has(PRECEDENCE) {
                state.index += 1;
            } else if c == unit('"') && self.has(PHRASE) {
                self.consume_phrase(state)?;
            } else if c == unit('+') && self.has(AND) {
                if state.current.is_none() && state.top.is_some() {
                    state.current = Some(Occur::Must);
                }
                state.index += 1;
            } else if c == unit('|') && self.has(OR) {
                if state.current.is_none() && state.top.is_some() {
                    state.current = Some(Occur::Should);
                }
                state.index += 1;
            } else if c == unit('-') && self.has(NOT) {
                state.not += 1;
                state.index += 1;
                continue;
            } else if matches!(c, 0x20 | 0x09 | 0x0a | 0x0d) && self.has(WHITESPACE) {
                state.index += 1;
            } else {
                self.consume_token(state)?;
            }
            state.not = 0;
        }
        Ok(())
    }

    fn consume_sub_query(&mut self, state: &mut State) -> Result<(), Failure> {
        state.index += 1;
        let start = state.index;
        let mut precedence = 1;
        let mut escaped = false;
        while state.index < state.length {
            self.tick()?;
            if !escaped {
                let c = self.at(state.index);
                if c == unit('\\') && self.has(ESCAPE) {
                    escaped = true;
                    state.index += 1;
                    continue;
                } else if c == unit('(') {
                    precedence += 1;
                } else if c == unit(')') {
                    precedence -= 1;
                    if precedence == 0 {
                        break;
                    }
                }
            }
            escaped = false;
            state.index += 1;
        }
        if state.index == state.length {
            state.index = start;
        } else if state.index == start {
            state.current = None;
            state.index += 1;
        } else {
            if self.depth >= MAX_GROUP_DEPTH {
                // ALLOC: error path; one message.
                return Err(unsupported(&format!(
                    "a [simple_query_string] nesting more than {MAX_GROUP_DEPTH} groups"
                )));
            }
            let mut sub = State::new(start, state.index);
            self.depth += 1;
            let parsed = self.parse_sub_query(&mut sub);
            self.depth -= 1;
            parsed?;
            self.build_query_tree(state, sub.top)?;
            state.index += 1;
        }
        Ok(())
    }

    fn consume_phrase(&mut self, state: &mut State) -> Result<(), Failure> {
        state.index += 1;
        let start = state.index;
        let mut copied = 0;
        let mut escaped = false;
        let mut has_slop = false;
        while state.index < state.length {
            self.tick()?;
            if !escaped {
                let c = self.at(state.index);
                if c == unit('\\') && self.has(ESCAPE) {
                    escaped = true;
                    state.index += 1;
                    continue;
                } else if c == unit('"') {
                    if state.length > state.index + 1
                        && self.at(state.index + 1) == unit('~')
                        && self.has(NEAR)
                    {
                        state.index += 1;
                        if state.length > state.index + 1 {
                            has_slop = true;
                        }
                    }
                    break;
                }
            }
            escaped = false;
            let unit_at = self.at(state.index);
            if let Some(slot) = self.buffer.get_mut(copied) {
                *slot = unit_at;
            }
            copied += 1;
            state.index += 1;
        }
        if state.index == state.length {
            state.index = start;
        } else if state.index == start {
            state.current = None;
            state.index += 1;
        } else {
            let phrase = String::from_utf16_lossy(self.buffer.get(..copied).unwrap_or(&[]));
            let slop = if has_slop {
                self.parse_fuzziness(state)?
            } else {
                0
            };
            let branch = self.new_phrase_query(&phrase, slop)?;
            self.build_query_tree(state, branch)?;
            state.index += 1;
        }
        Ok(())
    }

    fn consume_token(&mut self, state: &mut State) -> Result<(), Failure> {
        let mut copied = 0;
        let mut escaped = false;
        let mut prefix = false;
        let mut fuzzy = false;
        while state.index < state.length {
            self.tick()?;
            if !escaped {
                let c = self.at(state.index);
                if c == unit('\\') && self.has(ESCAPE) {
                    escaped = true;
                    prefix = false;
                    state.index += 1;
                    continue;
                } else if self.token_finished(state) {
                    break;
                } else if copied > 0 && c == unit('~') && self.has(FUZZY) {
                    fuzzy = true;
                    break;
                }
                prefix = copied > 0 && c == unit('*') && self.has(PREFIX);
            }
            escaped = false;
            let unit_at = self.at(state.index);
            if let Some(slot) = self.buffer.get_mut(copied) {
                *slot = unit_at;
            }
            copied += 1;
            state.index += 1;
        }
        if copied > 0 {
            let text = |end: usize| String::from_utf16_lossy(self.buffer.get(..end).unwrap_or(&[]));
            let branch = if fuzzy && self.has(FUZZY) {
                let token = text(copied);
                let fuzziness = self.parse_fuzziness(state)?.min(MAXIMUM_SUPPORTED_DISTANCE);
                if fuzziness == 0 {
                    self.new_default_query(&token)?
                } else {
                    self.new_fuzzy_query(&token, fuzziness)?
                }
            } else if prefix {
                let token = text(copied - 1);
                self.new_prefix_query(&token)?
            } else {
                let token = text(copied);
                self.new_default_query(&token)?
            };
            self.build_query_tree(state, branch)?;
        }
        Ok(())
    }

    fn build_query_tree(&mut self, state: &mut State, branch: Option<Node>) -> Result<(), Failure> {
        let Some(mut branch) = branch else {
            return Ok(());
        };
        if state.not % 2 == 1 {
            // ALLOC: one vec a group, at most MAX_CLAUSES entries.
            branch = Node::Bool(vec![
                (Occur::MustNot, branch),
                (Occur::Should, Node::MatchAll),
            ]);
        }
        match state.top.take() {
            None => state.top = Some(branch),
            Some(top) => {
                let op = *state.current.get_or_insert(self.operator);
                let mut parts = if state.previous == Some(op) {
                    match top {
                        Node::Bool(parts) => parts,
                        // ALLOC: one vec a group, at most MAX_CLAUSES entries.
                        other => vec![(op, other)],
                    }
                } else {
                    // ALLOC: one vec a group, at most MAX_CLAUSES entries.
                    vec![(op, top)]
                };
                if parts.len() >= MAX_CLAUSES {
                    return Err(Failure::plain(
                        "too_many_clauses",
                        // ALLOC: error path; one message.
                        format!("maxClauseCount is set to {MAX_CLAUSES}"),
                    ));
                }
                parts.push((op, branch));
                state.top = Some(Node::Bool(parts));
                state.previous = Some(op);
            }
        }
        state.current = None;
        Ok(())
    }

    fn parse_fuzziness(&mut self, state: &mut State) -> Result<i32, Failure> {
        if self.at(state.index) != unit('~') {
            return Ok(0);
        }
        // ALLOC: the digits of one fuzziness, at most the query.
        let mut digits: Vec<u16> = Vec::new();
        while state.index < state.length {
            self.tick()?;
            state.index += 1;
            if state.index < state.length {
                if self.token_finished(state) {
                    break;
                }
                digits.push(self.at(state.index));
            }
        }
        let fuzziness = if digits.is_empty() {
            2
        } else {
            parse_java_int(&digits).unwrap_or(0)
        };
        Ok(fuzziness.max(0))
    }

    fn token_finished(&self, state: &State) -> bool {
        let c = self.at(state.index);
        (c == unit('"') && self.has(PHRASE))
            || (c == unit('|') && self.has(OR))
            || (c == unit('+') && self.has(AND))
            || (c == unit('(') && self.has(PRECEDENCE))
            || (c == unit(')') && self.has(PRECEDENCE))
            || (matches!(c, 0x20 | 0x09 | 0x0a | 0x0d) && self.has(WHITESPACE))
    }
}

/// `Integer.parseInt` of ASCII digits with an optional sign; `None` for
/// what throws `NumberFormatException`.
fn parse_java_int(units: &[u16]) -> Option<i32> {
    let text: String = char::decode_utf16(units.iter().copied())
        .map(|r| r.unwrap_or('\u{fffd}'))
        // ALLOC: the query text of one fuzziness, at most the query.
        .collect();
    text.parse::<i32>().ok()
}
