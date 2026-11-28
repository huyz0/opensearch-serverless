//! Conditions: `WHERE` expressions, literals and the text-search functions.

use crate::Error;
use crate::ast::{CmpOp, Lit, Pred, Relevance};
use crate::lexer::Kind;
use crate::parser::{Parser, RELEVANCE};

impl Parser {
    pub(crate) fn enter(&mut self) -> Result<(), Error> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.max_depth {
            return Err(Error::bound("nesting depth", self.max_depth));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub(crate) fn or(&mut self, alias: Option<&str>) -> Result<Pred, Error> {
        self.enter()?;
        let mut parts = vec![self.and(alias)?];
        while self.eat_word("or") {
            parts.push(self.and(alias)?);
        }
        self.leave();
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Pred::Or(parts)
        })
    }

    fn and(&mut self, alias: Option<&str>) -> Result<Pred, Error> {
        let mut parts = vec![self.not(alias)?];
        while self.eat_word("and") {
            parts.push(self.not(alias)?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Pred::And(parts)
        })
    }

    fn not(&mut self, alias: Option<&str>) -> Result<Pred, Error> {
        if self.eat_word("not") {
            self.enter()?;
            let inner = self.not(alias)?;
            self.leave();
            return Ok(Pred::Not(Box::new(inner)));
        }
        self.atom(alias)
    }

    fn atom(&mut self, alias: Option<&str>) -> Result<Pred, Error> {
        if self.eat_sym("(") {
            if self.is_word("select") {
                return Err(Error::unsupported("a subquery is not supported"));
            }
            let inner = self.or(alias)?;
            self.expect_sym(")")?;
            return Ok(inner);
        }
        if let Some(Kind::Word(w)) = self.peek() {
            let lower = w.to_ascii_lowercase();
            if matches!(
                self.tokens.get(self.at.saturating_add(1)).map(|t| &t.kind),
                Some(Kind::Sym("("))
            ) {
                if RELEVANCE.contains(&lower.as_str()) {
                    self.bump();
                    return self.relevance(lower, alias);
                }
                return Err(Error::unsupported(format!(
                    "the function [{w}] is not supported yet"
                )));
            }
        }
        let field = self.field(alias)?;
        let negated = self.is_word("not") && self.tokens.get(self.at.saturating_add(1)).is_some();
        if negated {
            self.bump();
        }
        if self.eat_word("between") {
            let lo = self.literal()?;
            self.expect_word("and")?;
            let hi = self.literal()?;
            return Ok(Pred::Between(field, lo, hi, negated));
        }
        if self.eat_word("in") {
            self.expect_sym("(")?;
            if self.is_word("select") {
                return Err(Error::unsupported("a subquery is not supported"));
            }
            let mut vals = vec![self.literal()?];
            while self.eat_sym(",") {
                vals.push(self.literal()?);
            }
            self.expect_sym(")")?;
            return Ok(Pred::In(field, vals, negated));
        }
        if self.eat_word("like") {
            return match self.literal()? {
                Lit::Str(s) => Ok(Pred::Like(field, s, negated)),
                _ => self.fail(),
            };
        }
        if negated {
            return self.fail();
        }
        if self.eat_word("is") {
            let not = self.eat_word("not");
            self.expect_word("null")?;
            return Ok(Pred::IsNull(field, not));
        }
        let op = match self.peek() {
            Some(Kind::Sym("=")) => CmpOp::Eq,
            Some(Kind::Sym("<>" | "!=")) => CmpOp::Ne,
            Some(Kind::Sym("<")) => CmpOp::Lt,
            Some(Kind::Sym("<=")) => CmpOp::Le,
            Some(Kind::Sym(">")) => CmpOp::Gt,
            Some(Kind::Sym(">=")) => CmpOp::Ge,
            _ => return self.fail(),
        };
        self.bump();
        let lit = self.literal()?;
        Ok(Pred::Cmp(field, op, lit))
    }

    fn literal(&mut self) -> Result<Lit, Error> {
        let neg = self.eat_sym("-");
        let lit = match self.peek().cloned() {
            Some(Kind::Str(s)) if !neg => Lit::Str(s),
            Some(Kind::Int(n)) => Lit::Int(if neg { n.checked_neg().unwrap_or(n) } else { n }),
            Some(Kind::Float(f)) => Lit::Float(if neg { -f } else { f }),
            Some(Kind::Word(w)) if !neg && w.eq_ignore_ascii_case("true") => Lit::Bool(true),
            Some(Kind::Word(w)) if !neg && w.eq_ignore_ascii_case("false") => Lit::Bool(false),
            Some(Kind::Word(w)) if !neg && w.eq_ignore_ascii_case("null") => Lit::Null,
            Some(Kind::Word(w)) if w.eq_ignore_ascii_case("select") => {
                return Err(Error::unsupported("a subquery is not supported"));
            }
            _ => {
                return Err(Error::unsupported(
                    "only a literal can be compared with a field; an expression needs script pushdown, a later slice",
                ));
            }
        };
        self.bump();
        for op in ["+", "/", "%"] {
            if self.is_sym(op) {
                return Err(Error::unsupported(
                    "an arithmetic expression is not supported yet",
                ));
            }
        }
        Ok(lit)
    }

    fn relevance(&mut self, func: String, alias: Option<&str>) -> Result<Pred, Error> {
        self.expect_sym("(")?;
        let multi = func == "multi_match" || func == "query_string";
        let mut fields = Vec::new();
        if multi {
            self.expect_sym("[")?;
            loop {
                let (name, boost) = match self.peek().cloned() {
                    Some(Kind::Str(s)) => {
                        self.bump();
                        match s.split_once('^') {
                            Some((n, b)) => (
                                n.to_owned(),
                                b.parse::<f64>()
                                    .map_err(|_| Error::syntax(&s, self.pos()))?,
                            ),
                            None => (s, 1.0),
                        }
                    }
                    Some(Kind::Word(_) | Kind::Quoted(_)) => (self.field(alias)?, 1.0),
                    _ => return self.fail(),
                };
                fields.push((name, boost));
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym("]")?;
        } else {
            fields.push((self.field(alias)?, 1.0));
        }
        self.expect_sym(",")?;
        let query = match self.literal()? {
            Lit::Str(s) => s,
            _ => return self.fail(),
        };
        let mut options = Vec::new();
        while self.eat_sym(",") {
            let name = match self.peek().cloned() {
                Some(Kind::Word(w)) => {
                    self.bump();
                    w.to_ascii_lowercase()
                }
                _ => return self.fail(),
            };
            self.expect_sym("=")?;
            options.push((name, self.literal()?));
        }
        self.expect_sym(")")?;
        Ok(Pred::Rel(Relevance {
            func,
            fields,
            query,
            options,
        }))
    }
}
