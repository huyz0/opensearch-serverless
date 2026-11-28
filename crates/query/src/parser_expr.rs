//! Scalar expressions in a select list: arithmetic, function calls and `CASE`.

use crate::Error;
use crate::ast::Lit;
use crate::expr::{Arith, Expr, FUNCTIONS, pred_supported};
use crate::lexer::Kind;
use crate::parser::Parser;

impl Parser {
    /// Whether the select item starting here is an expression rather than a plain field.
    pub(crate) fn looks_like_expr(&self) -> bool {
        let next = self.tokens.get(self.at.saturating_add(1)).map(|t| &t.kind);
        match self.peek() {
            Some(Kind::Int(_) | Kind::Float(_) | Kind::Str(_) | Kind::Sym("(" | "-")) => true,
            Some(Kind::Word(w)) if w.eq_ignore_ascii_case("case") => true,
            Some(Kind::Word(_) | Kind::Quoted(_)) => {
                matches!(next, Some(Kind::Sym("(" | "+" | "-" | "*" | "/" | "%")))
            }
            _ => false,
        }
    }

    pub(crate) fn expr(&mut self) -> Result<Expr, Error> {
        self.enter()?;
        let mut left = self.term()?;
        loop {
            let op = if self.eat_sym("+") {
                Arith::Add
            } else if self.eat_sym("-") {
                Arith::Sub
            } else {
                break;
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.term()?));
        }
        self.leave();
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr, Error> {
        let mut left = self.primary()?;
        loop {
            let op = if self.eat_sym("*") {
                Arith::Mul
            } else if self.eat_sym("/") {
                Arith::Div
            } else if self.eat_sym("%") {
                Arith::Rem
            } else {
                break;
            };
            left = Expr::Bin(op, Box::new(left), Box::new(self.primary()?));
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        if self.eat_sym("(") {
            let inner = self.expr()?;
            self.expect_sym(")")?;
            return Ok(inner);
        }
        match self.peek().cloned() {
            Some(Kind::Int(_) | Kind::Float(_) | Kind::Str(_)) | Some(Kind::Sym("-")) => {
                Ok(Expr::Lit(self.literal_in(false)?))
            }
            Some(Kind::Word(w)) if w.eq_ignore_ascii_case("case") => {
                self.bump();
                self.case()
            }
            Some(Kind::Word(w)) if w.eq_ignore_ascii_case("null") => {
                self.bump();
                Ok(Expr::Lit(Lit::Null))
            }
            Some(Kind::Word(w))
                if matches!(
                    self.tokens.get(self.at.saturating_add(1)).map(|t| &t.kind),
                    Some(Kind::Sym("("))
                ) =>
            {
                self.bump();
                self.bump();
                self.call(&w.to_ascii_lowercase())
            }
            _ => Ok(Expr::Field(self.path_in(false)?)),
        }
    }

    fn case(&mut self) -> Result<Expr, Error> {
        let mut branches = Vec::new();
        while self.eat_word("when") {
            let cond = self.or(None)?;
            if !pred_supported(&cond) {
                return Err(Error::unsupported(
                    "only a comparison, AND, OR, NOT and IS NULL are supported inside CASE",
                ));
            }
            self.expect_word("then")?;
            branches.push((cond, self.expr()?));
        }
        if branches.is_empty() {
            return self.fail();
        }
        let other = if self.eat_word("else") {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_word("end")?;
        Ok(Expr::Case(branches, other))
    }

    /// The arguments of `name(`, the opening parenthesis already read.
    fn call(&mut self, name: &str) -> Result<Expr, Error> {
        if name == "if" {
            let cond = self.or(None)?;
            if !pred_supported(&cond) {
                return Err(Error::unsupported(
                    "only a comparison, AND, OR, NOT and IS NULL are supported inside IF",
                ));
            }
            self.expect_sym(",")?;
            let (a, b) = (self.expr()?, {
                self.expect_sym(",")?;
                self.expr()?
            });
            self.expect_sym(")")?;
            return Ok(Expr::Case(vec![(cond, a)], Some(Box::new(b))));
        }
        let mut args = Vec::new();
        if !self.is_sym(")") {
            args.push(self.expr()?);
            while self.eat_sym(",") {
                args.push(self.expr()?);
            }
        }
        self.expect_sym(")")?;
        let known = if name == "concat" {
            !args.is_empty()
        } else {
            FUNCTIONS
                .iter()
                .any(|(n, k)| *n == name && *k == args.len())
        };
        if !known {
            return Err(Error::unsupported(format!(
                "the function [{name}] is not supported yet"
            )));
        }
        Ok(Expr::Call(name.to_owned(), args))
    }
}
