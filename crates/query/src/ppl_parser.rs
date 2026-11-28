//! The pipe language: `source=<names> | command | command ...`, parsed to the same
//! [`Select`] the SQL parser gives, so one planner serves both languages.

use crate::ast::{Agg, Dialect, Item, Order, Pred, Select};
use crate::expr::Expr;
use crate::lexer::{Kind, lex};
use crate::parser::Parser;
use crate::{Error, Limits};

/// Commands the plugin has and this slice does not serve.
const LATER: [&str; 15] = [
    "parse",
    "dedup",
    "join",
    "lookup",
    "trendline",
    "eventstats",
    "streamstats",
    "fillnull",
    "expand",
    "flatten",
    "patterns",
    "ml",
    "kmeans",
    "describe",
    "grok",
];

/// Parses one PPL statement.
///
/// # Errors
///
/// A bound passed, a syntax error, or a command this slice does not serve.
pub fn parse_ppl(src: &str, limits: &Limits) -> Result<Select, Error> {
    if src.trim().is_empty() {
        return Err(Error::empty());
    }
    let tokens = lex(src, limits)?;
    let mut p = Parser {
        tokens,
        at: 0,
        depth: 0,
        max_depth: limits.max_depth,
        end: src.len(),
        src: src.to_owned(),
        ppl: true,
    };
    p.ppl().map_err(|e| {
        if e.is_syntax() {
            p.ppl_error("Expecting a valid command, field or value")
        } else {
            e.into_ppl()
        }
    })
}

impl Parser {
    /// The parse error at the token the cursor is on, or at the end.
    fn ppl_error(&self, extra: &str) -> Error {
        let (start, end) = match self.tokens.get(self.at) {
            Some(t) => {
                let next = self
                    .tokens
                    .get(self.at.saturating_add(1))
                    .map_or(self.end, |n| n.pos);
                // The token ends before the whitespace that follows it.
                let text = self.src.get(t.pos..next).unwrap_or("");
                (t.pos, t.pos.saturating_add(text.trim_end().len()))
            }
            None => (self.end, self.end),
        };
        Error::ppl_syntax(&self.src, start, end, extra)
    }

    fn ppl_word(&mut self, w: &str) -> Result<(), Error> {
        if self.eat_word(w) {
            Ok(())
        } else {
            Err(self.ppl_error("Expecting a valid command, field or value"))
        }
    }

    fn ppl(&mut self) -> Result<Select, Error> {
        self.eat_word("search");
        self.ppl_word("source")?;
        self.expect_sym("=")?;
        let mut from = Vec::new();
        loop {
            match self.peek().cloned() {
                Some(Kind::Word(w) | Kind::Quoted(w)) => {
                    self.bump();
                    from.push(w);
                }
                _ => return self.fail(),
            }
            if !self.eat_sym(",") {
                break;
            }
        }
        let mut sel = Select {
            dialect: Dialect::Ppl,
            items: vec![Item::Star],
            from,
            filter: None,
            distinct: false,
            group: Vec::new(),
            having: None,
            order: Vec::new(),
            limit: None,
            offset: 0,
        };
        let mut renames: Vec<(String, String)> = Vec::new();
        let mut evals: Vec<(String, Expr, String)> = Vec::new();
        while self.eat_sym("|") {
            self.ppl_command(&mut sel, &mut renames, &mut evals)?;
        }
        if self.at < self.tokens.len() {
            return Err(self.ppl_error("Expecting a valid command, field or value"));
        }
        Ok(sel)
    }

    fn ppl_command(
        &mut self,
        sel: &mut Select,
        renames: &mut Vec<(String, String)>,
        evals: &mut Vec<(String, Expr, String)>,
    ) -> Result<(), Error> {
        let Some(Kind::Word(w)) = self.peek().cloned() else {
            return Err(self.ppl_error("Expecting a command"));
        };
        let cmd = w.to_ascii_lowercase();
        match cmd.as_str() {
            "where" => {
                self.bump();
                let p = self.or(None)?;
                sel.filter = Some(match sel.filter.take() {
                    None => p,
                    Some(Pred::And(mut v)) => {
                        v.push(p);
                        Pred::And(v)
                    }
                    Some(old) => Pred::And(vec![old, p]),
                });
            }
            "fields" => {
                self.bump();
                let minus = if self.eat_sym("-") {
                    true
                } else {
                    !self.eat_sym("+") && false
                };
                let names = self.ppl_names()?;
                sel.items = if minus {
                    vec![Item::StarExcept(names)]
                } else {
                    names
                        .into_iter()
                        .map(|n| match evals.iter().find(|(e, ..)| *e == n) {
                            Some((_, expr, text)) => {
                                Item::Expr(expr.clone(), Some(n), text.clone())
                            }
                            None => renamed(&n, renames),
                        })
                        .collect()
                };
            }
            "eval" => {
                self.bump();
                loop {
                    let Some(Kind::Word(name) | Kind::Quoted(name)) = self.peek().cloned() else {
                        return Err(self.ppl_error("Expecting a name"));
                    };
                    self.bump();
                    self.expect_sym("=")?;
                    let start = self.tokens.get(self.at).map_or(self.end, |t| t.pos);
                    let expr = self.expr()?;
                    let stop = self.tokens.get(self.at).map_or(self.end, |t| t.pos);
                    let text = self.src.get(start..stop).unwrap_or("").trim().to_owned();
                    sel.items
                        .push(Item::Expr(expr.clone(), Some(name.clone()), text.clone()));
                    evals.retain(|(e, ..)| *e != name);
                    evals.push((name, expr, text));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            }
            "sort" => {
                self.bump();
                loop {
                    let desc = self.eat_sym("-");
                    if !desc {
                        self.eat_sym("+");
                    }
                    let field = self.path()?;
                    let field = renames
                        .iter()
                        .find(|(_, to)| *to == field)
                        .map_or(field, |(f, _)| f.clone());
                    sel.order.push(Order {
                        field,
                        desc,
                        nulls_first: None,
                    });
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            }
            "head" => {
                self.bump();
                sel.limit = Some(if matches!(self.peek(), Some(Kind::Int(_))) {
                    self.uint()?
                } else {
                    10
                });
                if self.eat_word("from") {
                    sel.offset = self.uint()?;
                }
            }
            "rename" => {
                self.bump();
                loop {
                    let from = self.path()?;
                    self.ppl_word("as")?;
                    let Some(Kind::Word(to) | Kind::Quoted(to)) = self.peek().cloned() else {
                        return Err(self.ppl_error("Expecting a name"));
                    };
                    self.bump();
                    renames.push((from, to));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
            }
            "stats" => {
                self.bump();
                let mut items = Vec::new();
                loop {
                    let Some(agg) = self.agg_call_ppl()? else {
                        return Err(self.ppl_error("Expecting an aggregate"));
                    };
                    let alias = self.alias_for_item()?;
                    items.push(Item::Agg(agg, alias));
                    if !self.eat_sym(",") {
                        break;
                    }
                }
                if self.eat_word("by") {
                    sel.group = self.ppl_names()?;
                    items.extend(sel.group.iter().map(|g| Item::Field(g.clone(), None)));
                }
                sel.items = items;
            }
            "top" | "rare" => {
                self.bump();
                let n = if matches!(self.peek(), Some(Kind::Int(_))) {
                    self.uint()?
                } else {
                    10
                };
                let field = self.path()?;
                sel.items = vec![
                    Item::Field(field.clone(), None),
                    Item::Agg(
                        Agg {
                            func: "count".into(),
                            field: None,
                        },
                        Some("count".into()),
                    ),
                ];
                sel.group = vec![field];
                sel.order = vec![Order {
                    field: "count".into(),
                    desc: cmd == "top",
                    nulls_first: None,
                }];
                sel.limit = Some(n);
            }
            other if LATER.contains(&other) => {
                return Err(Error::unsupported(format!(
                    "the PPL command [{other}] is not supported yet"
                )));
            }
            _ => {
                let word = w.clone();
                return Err(self.ppl_error(&format!("extraneous input '{word}' expecting <EOF>")));
            }
        }
        Ok(())
    }

    /// `a, b` or `a b`: field names.
    fn ppl_names(&mut self) -> Result<Vec<String>, Error> {
        let mut out = vec![self.path()?];
        loop {
            let comma = self.eat_sym(",");
            if matches!(self.peek(), Some(Kind::Word(w)) if !comma && w.eq_ignore_ascii_case("by"))
            {
                break;
            }
            if matches!(self.peek(), Some(Kind::Word(_) | Kind::Quoted(_))) {
                out.push(self.path()?);
            } else if comma {
                return self.fail();
            } else {
                break;
            }
        }
        Ok(out)
    }

    /// An aggregate call where `count()` is `count(*)`.
    fn agg_call_ppl(&mut self) -> Result<Option<Agg>, Error> {
        let kinds: Vec<Option<Kind>> = (0..3)
            .map(|i| {
                self.tokens
                    .get(self.at.saturating_add(i))
                    .map(|t| t.kind.clone())
            })
            .collect();
        if let [
            Some(Kind::Word(w)),
            Some(Kind::Sym("(")),
            Some(Kind::Sym(")")),
        ] = kinds.as_slice()
            && w.eq_ignore_ascii_case("count")
        {
            self.bump();
            self.bump();
            self.bump();
            return Ok(Some(Agg {
                func: "count".into(),
                field: None,
            }));
        }
        self.agg_call(None)
    }
}

/// A field as `fields` names it: through a `rename`, the original with its new name.
fn renamed(name: &str, renames: &[(String, String)]) -> Item {
    match renames.iter().find(|(_, to)| to == name) {
        Some((from, to)) => Item::Field(from.clone(), Some(to.clone())),
        None => Item::Field(name.to_owned(), None),
    }
}
