//! The aggregate parts of a `SELECT`: calls in the select list, `GROUP BY`, `HAVING`.

use crate::Error;
use crate::ast::{Agg, CmpOp, Having, Item, Lit};
use crate::lexer::Kind;
use crate::parser::Parser;

const FUNCS: [&str; 5] = ["count", "sum", "avg", "min", "max"];

impl Parser {
    /// An aggregate call at the cursor, or `None` when the word is not one.
    pub(crate) fn agg_call(&mut self, alias: Option<&str>) -> Result<Option<Agg>, Error> {
        let Some(Kind::Word(w)) = self.peek().cloned() else {
            return Ok(None);
        };
        let func = w.to_ascii_lowercase();
        let call =
            self.tokens.get(self.at.saturating_add(1)).map(|t| &t.kind) == Some(&Kind::Sym("("));
        if !call || !FUNCS.contains(&func.as_str()) {
            return Ok(None);
        }
        self.bump();
        self.bump();
        let field = if self.eat_sym("*") {
            if func != "count" {
                return self.fail();
            }
            None
        } else if self.is_word("distinct") {
            return Err(Error::unsupported(
                "COUNT(DISTINCT ...) is not supported yet",
            ));
        } else {
            Some(self.field(alias)?)
        };
        self.expect_sym(")")?;
        Ok(Some(Agg { func, field }))
    }

    /// The select item for an aggregate, with its alias.
    pub(crate) fn agg_item(&mut self, agg: Agg) -> Result<Item, Error> {
        let alias = self.alias_for_item()?;
        Ok(Item::Agg(agg, alias))
    }

    /// `GROUP BY a, b`, when written.
    pub(crate) fn group_by(&mut self, alias: Option<&str>) -> Result<Vec<String>, Error> {
        let mut out = Vec::new();
        if self.eat_word("group") {
            self.expect_word("by")?;
            loop {
                out.push(self.field(alias)?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// `HAVING <aggregate or alias> <op> <literal>`, when written.
    pub(crate) fn having(&mut self, alias: Option<&str>) -> Result<Option<Having>, Error> {
        if !self.eat_word("having") {
            return Ok(None);
        }
        let target = match self.agg_call(alias)? {
            Some(a) => a.name(),
            None => self.path()?,
        };
        let op = self.cmp_op()?;
        let lit = self.literal()?;
        if matches!(lit, Lit::Null) {
            return self.fail();
        }
        Ok(Some(Having { target, op, lit }))
    }

    fn cmp_op(&mut self) -> Result<CmpOp, Error> {
        for (sym, op) in [
            ("=", CmpOp::Eq),
            ("<>", CmpOp::Ne),
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            (">", CmpOp::Gt),
        ] {
            if self.eat_sym(sym) {
                return Ok(op);
            }
        }
        self.fail()
    }
}
