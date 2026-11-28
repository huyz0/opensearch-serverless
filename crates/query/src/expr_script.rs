//! A comparison with a function or arithmetic in it, lowered to one Painless `script` query.
//!
//! The source is generated from a fixed table of functions and holds no literal of the
//! statement: every literal is a `params` entry, and a field name is spliced in only after the
//! mapping has resolved it and it has no quote or backslash.

use serde_json::{Map, Value, json};

use crate::ast::{CmpOp, Lit};
use crate::expr::{Arith, Expr};
use crate::plan::{exact_name, lookup};
use crate::{Error, Mapping};

/// The Painless script query for `left <op> right`: false for a document missing a field it reads.
pub(crate) fn query(left: &Expr, op: CmpOp, right: &Expr, m: &Mapping) -> Result<Value, Error> {
    let mut g = Gen {
        m,
        params: Map::new(),
        guards: Vec::new(),
    };
    let (a, b) = (g.expr(left)?, g.expr(right)?);
    let text = |e: &Expr| matches!(e.sql_type(m).as_str(), "keyword" | "text");
    let ordered = !matches!(op, CmpOp::Eq | CmpOp::Ne);
    if ordered && (text(left) || text(right)) {
        return Err(Error::unsupported(
            "a string in a WHERE expression can be compared only with = or <>",
        ));
    }
    let sym = match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    };
    let mut source = g.guards.join(" && ");
    if !source.is_empty() {
        source.push_str(" && ");
    }
    source.push_str(&format!("({a} {sym} {b})"));
    Ok(json!({"script": {"script": {
        "source": source,
        "lang": "painless",
        "params": g.params,
    }}}))
}

struct Gen<'a> {
    m: &'a Mapping,
    params: Map<String, Value>,
    guards: Vec<String>,
}

fn numeric(t: &str) -> bool {
    matches!(
        t,
        "byte" | "short" | "integer" | "long" | "float" | "double"
    )
}

impl Gen<'_> {
    fn expr(&mut self, e: &Expr) -> Result<String, Error> {
        match e {
            Expr::Lit(l) => self.literal(l),
            Expr::Field(f) => self.field(f),
            Expr::Bin(op, a, b) => {
                for x in [a, b] {
                    self.want(x, true, "arithmetic")?;
                }
                if matches!(op, Arith::Div | Arith::Rem)
                    && !matches!(&**b, Expr::Lit(Lit::Int(n)) if *n != 0)
                    && !matches!(&**b, Expr::Lit(Lit::Float(f)) if *f != 0.0)
                {
                    return Err(Error::unsupported(
                        "division in a WHERE expression needs a non-zero literal divisor",
                    ));
                }
                let sym = match op {
                    Arith::Add => "+",
                    Arith::Sub => "-",
                    Arith::Mul => "*",
                    Arith::Div => "/",
                    Arith::Rem => "%",
                };
                Ok(format!("({} {sym} {})", self.expr(a)?, self.expr(b)?))
            }
            Expr::Call(name, args) => self.call(name, args),
            Expr::Case(..) => Err(Error::unsupported(
                "CASE and IF in a WHERE expression are not supported yet",
            )),
        }
    }

    /// Checks an operand's type: numeric when `num`, a string otherwise.
    fn want(&self, e: &Expr, num: bool, what: &str) -> Result<(), Error> {
        let t = e.sql_type(self.m);
        let is_num = numeric(&t);
        let is_text = matches!(t.as_str(), "keyword" | "text");
        if (num && is_num) || (!num && is_text) {
            return Ok(());
        }
        Err(Error::unsupported(format!(
            "{what} in a WHERE expression needs a {} operand",
            if num { "numeric" } else { "string" }
        )))
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<String, Error> {
        let one = |g: &mut Self, num: bool| -> Result<String, Error> {
            let a = args
                .first()
                .ok_or_else(|| Error::unsupported("no argument"))?;
            g.want(a, num, name)?;
            g.expr(a)
        };
        match name {
            "abs" => Ok(format!("Math.abs({})", one(self, true)?)),
            "upper" => Ok(format!("{}.toUpperCase()", one(self, false)?)),
            "lower" => Ok(format!("{}.toLowerCase()", one(self, false)?)),
            "length" => Ok(format!("{}.length()", one(self, false)?)),
            "concat" => {
                let mut parts = Vec::new();
                for a in args {
                    self.want(a, false, name)?;
                    parts.push(self.expr(a)?);
                }
                Ok(format!("({})", parts.join(" + ")))
            }
            other => Err(Error::unsupported(format!(
                "the function [{other}] is not supported in a WHERE expression yet"
            ))),
        }
    }

    fn literal(&mut self, l: &Lit) -> Result<String, Error> {
        let v = match l {
            Lit::Str(s) => json!(s),
            Lit::Int(n) => json!(n),
            Lit::Float(f) => json!(f),
            Lit::Bool(b) => json!(b),
            Lit::Null => {
                return Err(Error::unsupported("compare with NULL using IS NULL"));
            }
        };
        let key = format!("p{}", self.params.len());
        self.params.insert(key.clone(), v);
        Ok(format!("params.{key}"))
    }

    fn field(&mut self, name: &str) -> Result<String, Error> {
        let f = lookup(self.m, name)?;
        if matches!(f.es_type.as_str(), "date" | "date_nanos") {
            return Err(Error::unsupported(
                "a date field in a WHERE expression is not supported yet",
            ));
        }
        let exact = exact_name(name, f, "compare")?;
        if exact.contains(['\'', '\\']) {
            return Err(Error::unsupported(format!(
                "the field [{name}] cannot be read in a WHERE expression"
            )));
        }
        let read = format!("doc['{exact}']");
        let guard = format!("{read}.size() > 0");
        if !self.guards.contains(&guard) {
            self.guards.push(guard);
        }
        Ok(format!("{read}.value"))
    }
}
