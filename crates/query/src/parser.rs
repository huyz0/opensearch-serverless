//! A recursive-descent parser for the statements of slice 1.

use crate::ast::{Dialect, Item, Order, Select, Statement};
use crate::lexer::{Kind, Token, lex};
use crate::{Error, Limits};

pub(crate) const RELEVANCE: [&str; 8] = [
    "match",
    "match_query",
    "simple_query_string",
    "match_phrase",
    "match_phrase_prefix",
    "match_bool_prefix",
    "multi_match",
    "query_string",
];

/// Words that are never a bare field name (a quoted one is).
const RESERVED: [&str; 8] = [
    "select", "from", "where", "group", "order", "by", "having", "limit",
];

const STOP: [&str; 14] = [
    "where", "order", "limit", "group", "having", "join", "union", "inner", "left", "right",
    "cross", "offset", "on", "full",
];

/// Parses one statement.
///
/// # Errors
///
/// A bound passed, a syntax error, or a construct the planner cannot push.
pub fn parse(sql: &str, limits: &Limits) -> Result<Statement, Error> {
    let first = sql.split_whitespace().next().map(str::to_ascii_lowercase);
    match first.as_deref() {
        None => return Err(Error::empty()),
        Some("select" | "show" | "describe") => {}
        Some(_) if sql.len() <= limits.max_statement_bytes => {
            return Err(Error::not_a_query(sql.trim()));
        }
        Some(_) => {}
    }
    let tokens = lex(sql, limits)?;
    let mut p = Parser {
        tokens,
        at: 0,
        depth: 0,
        max_depth: limits.max_depth,
        end: sql.len(),
        src: sql.to_owned(),
        ppl: false,
    };
    let stmt = p.statement()?;
    if let Some(t) = p.tokens.get(p.at) {
        return Err(Error::syntax(&describe(&t.kind), t.pos));
    }
    Ok(stmt)
}

pub(crate) fn describe(k: &Kind) -> String {
    match k {
        Kind::Word(w) | Kind::Quoted(w) | Kind::Str(w) => w.clone(),
        Kind::Int(n) => n.to_string(),
        Kind::Float(f) => f.to_string(),
        Kind::Sym(s) => (*s).to_owned(),
    }
}

pub(crate) struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) at: usize,
    pub(crate) depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) end: usize,
    /// The statement text, kept by the PPL parser for its error texts.
    pub(crate) src: String,
    /// Whether the PPL predicate functions are accepted.
    pub(crate) ppl: bool,
}

impl Parser {
    pub(crate) fn peek(&self) -> Option<&Kind> {
        self.tokens.get(self.at).map(|t| &t.kind)
    }

    pub(crate) fn pos(&self) -> usize {
        self.tokens.get(self.at).map_or(self.end, |t| t.pos)
    }

    pub(crate) fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    pub(crate) fn fail<T>(&self) -> Result<T, Error> {
        match self.tokens.get(self.at) {
            Some(t) => Err(Error::syntax(&describe(&t.kind), t.pos)),
            None => Err(Error::syntax("<EOF>", self.end)),
        }
    }

    pub(crate) fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Kind::Word(x)) if x.eq_ignore_ascii_case(w))
    }

    pub(crate) fn eat_word(&mut self, w: &str) -> bool {
        let hit = self.is_word(w);
        if hit {
            self.bump();
        }
        hit
    }

    pub(crate) fn expect_word(&mut self, w: &str) -> Result<(), Error> {
        if self.eat_word(w) {
            Ok(())
        } else {
            self.fail()
        }
    }

    pub(crate) fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Kind::Sym(x)) if *x == s)
    }

    pub(crate) fn eat_sym(&mut self, s: &str) -> bool {
        let hit = self.is_sym(s);
        if hit {
            self.bump();
        }
        hit
    }

    pub(crate) fn expect_sym(&mut self, s: &str) -> Result<(), Error> {
        if self.eat_sym(s) { Ok(()) } else { self.fail() }
    }

    fn statement(&mut self) -> Result<Statement, Error> {
        if self.eat_word("select") {
            return self.select().map(|s| Statement::Select(Box::new(s)));
        }
        if self.eat_word("show") {
            self.expect_word("tables")?;
            self.expect_word("like")?;
            return Ok(Statement::ShowTables {
                pattern: self.pattern()?,
            });
        }
        if self.eat_word("describe") {
            self.expect_word("tables")?;
            self.expect_word("like")?;
            let pattern = self.pattern()?;
            let columns = if self.eat_word("columns") {
                self.expect_word("like")?;
                Some(self.pattern()?)
            } else if self.eat_word("column") {
                // Recorded: stock reads the singular form and filters nothing (`describe-columns`).
                self.expect_word("like")?;
                self.pattern()?;
                None
            } else {
                None
            };
            return Ok(Statement::DescribeTables { pattern, columns });
        }
        match self.peek() {
            Some(Kind::Word(w))
                if [
                    "delete", "insert", "update", "create", "drop", "alter", "with", "explain",
                ]
                .iter()
                .any(|k| w.eq_ignore_ascii_case(k)) =>
            {
                Err(Error::unsupported(format!(
                    "[{}] is not supported: the SQL endpoint is read only",
                    w.to_ascii_uppercase()
                )))
            }
            _ => self.fail(),
        }
    }

    /// A `LIKE` pattern: a string or quoted name, or the plugin's bare form, which runs to the
    /// next space (`show tables like logs-%`).
    fn pattern(&mut self) -> Result<String, Error> {
        match self.peek().cloned() {
            Some(Kind::Str(s) | Kind::Quoted(s)) => {
                self.bump();
                Ok(s)
            }
            Some(Kind::Word(_) | Kind::Sym("%")) => {
                let start = self.tokens.get(self.at).map_or(0, |t| t.pos);
                let rest = self.src.get(start..).unwrap_or_default();
                let len = rest
                    .find(|c: char| c.is_whitespace() || c == ';')
                    .unwrap_or(rest.len());
                let text = rest.get(..len).unwrap_or_default().to_owned();
                let end = start.saturating_add(len);
                while self.tokens.get(self.at).is_some_and(|t| t.pos < end) {
                    self.bump();
                }
                Ok(text)
            }
            _ => self.fail(),
        }
    }

    fn select(&mut self) -> Result<Select, Error> {
        let distinct = self.eat_word("distinct");
        // ALLOC: bounded by the token limit.
        let mut items = Vec::new();
        loop {
            items.push(self.item()?);
            if !self.eat_sym(",") {
                break;
            }
        }
        if !self.is_word("from") && self.constant_items(&items) {
            return Ok(Select {
                dialect: Dialect::Sql,
                items,
                from: Vec::new(),
                filter: None,
                distinct,
                group: Vec::new(),
                having: None,
                order: Vec::new(),
                limit: None,
                offset: 0,
            });
        }
        self.expect_word("from")?;
        let from = self.table()?;
        let alias = self.alias()?;
        if self.is_sym(",") {
            return Err(Error::unsupported(
                "selecting from several tables (a join) is not supported",
            ));
        }
        for stop in ["join", "inner", "left", "right", "cross", "full", "union"] {
            if self.is_word(stop) {
                return Err(Error::unsupported(format!(
                    "[{}] is not supported",
                    stop.to_ascii_uppercase()
                )));
            }
        }
        let mut sel = Select {
            dialect: Dialect::Sql,
            items,
            from: vec![from],
            filter: None,
            distinct,
            group: Vec::new(),
            having: None,
            order: Vec::new(),
            limit: None,
            offset: 0,
        };
        if self.eat_word("where") {
            sel.filter = Some(self.or(alias.as_deref())?);
        }
        sel.group = self.group_by(alias.as_deref())?;
        sel.having = self.having(alias.as_deref())?;
        if self.eat_word("order") {
            self.expect_word("by")?;
            loop {
                let field = self.field(alias.as_deref())?;
                let desc = if self.eat_word("desc") {
                    true
                } else {
                    self.eat_word("asc");
                    false
                };
                let missing = if self.eat_word("nulls") {
                    if self.eat_word("first") {
                        Some(true)
                    } else {
                        self.expect_word("last")?;
                        Some(false)
                    }
                } else {
                    None
                };
                sel.order.push(Order {
                    field,
                    desc,
                    nulls_first: missing,
                });
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        if self.eat_word("limit") {
            let first = self.uint()?;
            if self.eat_sym(",") {
                sel.offset = first;
                sel.limit = Some(self.uint()?);
            } else {
                sel.limit = Some(first);
                if self.eat_word("offset") {
                    sel.offset = self.uint()?;
                }
            }
        }
        for item in &mut sel.items {
            if let Item::Field(name, _) = item {
                *name = strip_alias(name, alias.as_deref());
            }
        }
        Ok(sel)
    }

    pub(crate) fn uint(&mut self) -> Result<u64, Error> {
        if let Some(Kind::Int(n)) = self.peek() {
            let n = *n;
            self.bump();
            return u64::try_from(n).map_err(|_| Error::syntax(&n.to_string(), self.pos()));
        }
        self.fail()
    }

    fn table(&mut self) -> Result<String, Error> {
        match self.peek().cloned() {
            Some(Kind::Word(w) | Kind::Quoted(w)) => {
                self.bump();
                if self.is_sym("(") {
                    return Err(Error::unsupported(
                        "a table function or subquery after FROM is not supported",
                    ));
                }
                Ok(w)
            }
            Some(Kind::Sym("(")) => {
                Err(Error::unsupported("a subquery after FROM is not supported"))
            }
            _ => self.fail(),
        }
    }

    fn alias(&mut self) -> Result<Option<String>, Error> {
        if self.eat_word("as") {
            return match self.peek().cloned() {
                Some(Kind::Word(w) | Kind::Quoted(w)) => {
                    self.bump();
                    Ok(Some(w))
                }
                _ => self.fail(),
            };
        }
        match self.peek().cloned() {
            Some(Kind::Quoted(w)) => {
                self.bump();
                Ok(Some(w))
            }
            Some(Kind::Word(w)) if !STOP.iter().any(|s| w.eq_ignore_ascii_case(s)) => {
                self.bump();
                Ok(Some(w))
            }
            _ => Ok(None),
        }
    }

    fn item(&mut self) -> Result<Item, Error> {
        if self.eat_sym("*") {
            return Ok(Item::Star);
        }
        if let Some(agg) = self.agg_call(None)? {
            return self.agg_item(agg);
        }
        if self.looks_like_expr() {
            let start = self.tokens.get(self.at).map_or(self.end, |t| t.pos);
            let expr = self.expr()?;
            let stop = self.tokens.get(self.at).map_or(self.end, |t| t.pos);
            let text = self.src.get(start..stop).unwrap_or("").trim().to_owned();
            let alias = self.alias_for_item()?;
            return Ok(Item::Expr(expr, alias, text));
        }
        let path = self.path()?;
        let alias = self.alias_for_item()?;
        Ok(Item::Field(path, alias))
    }

    pub(crate) fn alias_for_item(&mut self) -> Result<Option<String>, Error> {
        if self.eat_word("as") {
            return match self.peek().cloned() {
                Some(Kind::Word(w) | Kind::Quoted(w)) => {
                    self.bump();
                    Ok(Some(w))
                }
                _ => self.fail(),
            };
        }
        Ok(None)
    }

    /// A dotted field path; anything that looks like a call or an operator is refused.
    pub(crate) fn path(&mut self) -> Result<String, Error> {
        self.path_in(true)
    }

    /// A dotted path; `bare` refuses a trailing operator (an expression reads it itself).
    pub(crate) fn path_in(&mut self, bare: bool) -> Result<String, Error> {
        let mut out = match self.peek().cloned() {
            Some(Kind::Word(w)) if RESERVED.contains(&w.to_ascii_lowercase().as_str()) => {
                return self.fail();
            }
            Some(Kind::Word(w) | Kind::Quoted(w)) => {
                self.bump();
                w
            }
            _ => return self.fail(),
        };
        if self.is_sym("(") {
            return Err(Error::unsupported(format!(
                "the function [{out}] is not supported in this position yet"
            )));
        }
        while self.eat_sym(".") {
            match self.peek().cloned() {
                Some(Kind::Word(w) | Kind::Quoted(w)) => {
                    self.bump();
                    out.push('.');
                    out.push_str(&w);
                }
                _ => return self.fail(),
            }
        }
        for op in ["+", "/", "%"] {
            if bare && self.is_sym(op) {
                return Err(Error::unsupported(
                    "an arithmetic expression is not supported yet",
                ));
            }
        }
        if self.is_word("over") {
            return Err(Error::unsupported("window functions are not supported yet"));
        }
        Ok(out)
    }

    pub(crate) fn field(&mut self, alias: Option<&str>) -> Result<String, Error> {
        let p = self.path()?;
        Ok(strip_alias(&p, alias))
    }
}

/// Drops a leading table alias from a path (`a.age` with alias `a`).
pub(crate) fn strip_alias(path: &str, alias: Option<&str>) -> String {
    match (alias, path.split_once('.')) {
        (Some(a), Some((head, rest))) if head == a => rest.to_owned(),
        _ => path.to_owned(),
    }
}
