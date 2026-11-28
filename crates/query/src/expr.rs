//! Scalar expressions of a select list: their tree, their type and their value over a row.

use serde_json::{Value, json};

use crate::Mapping;
use crate::ast::{CmpOp, Lit, Pred};

/// A binary arithmetic operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
}

/// One scalar expression.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A field, by dotted path.
    Field(String),
    /// A literal.
    Lit(Lit),
    /// A function call, name in lower case.
    Call(String, Vec<Expr>),
    /// Arithmetic over two expressions.
    Bin(Arith, Box<Expr>, Box<Expr>),
    /// `CASE WHEN c THEN a ... ELSE e END`; `IF(c, a, b)` is the one-branch form.
    Case(Vec<(Pred, Expr)>, Option<Box<Expr>>),
}

/// The functions and the number of arguments each takes.
pub(crate) const FUNCTIONS: &[(&str, usize)] = &[
    ("abs", 1),
    ("upper", 1),
    ("lower", 1),
    ("length", 1),
    ("ifnull", 2),
    ("year", 1),
    ("month", 1),
    ("date_format", 2),
];

impl Expr {
    /// Every field the expression reads, each once and in order of first use.
    pub(crate) fn fields(&self, out: &mut Vec<String>) {
        match self {
            Self::Field(f) => {
                if !out.contains(f) {
                    out.push(f.clone());
                }
            }
            Self::Lit(_) => {}
            Self::Call(_, args) => args.iter().for_each(|a| a.fields(out)),
            Self::Bin(_, a, b) => {
                a.fields(out);
                b.fields(out);
            }
            Self::Case(branches, other) => {
                for (c, e) in branches {
                    pred_fields(c, out);
                    e.fields(out);
                }
                if let Some(e) = other {
                    e.fields(out);
                }
            }
        }
    }

    /// The type name the plugin reports for the expression's column.
    pub(crate) fn sql_type(&self, mapping: &Mapping) -> String {
        match self {
            Self::Field(f) => mapping
                .field(f)
                .map_or("keyword", |x| x.sql_type())
                .to_owned(),
            Self::Lit(Lit::Int(_)) => "integer".into(),
            Self::Lit(Lit::Float(_)) => "double".into(),
            Self::Lit(Lit::Bool(_)) => "boolean".into(),
            Self::Lit(_) => "keyword".into(),
            Self::Bin(_, a, b) => {
                let (x, y) = (a.sql_type(mapping), b.sql_type(mapping));
                let rank = |t: &str| match t {
                    "double" | "float" => 3,
                    "long" => 2,
                    _ => 1,
                };
                match rank(&x).max(rank(&y)) {
                    3 => "double",
                    2 => "long",
                    _ => "integer",
                }
                .to_owned()
            }
            Self::Call(name, args) => match name.as_str() {
                "abs" | "ifnull" => args
                    .first()
                    .map_or("keyword".into(), |a| a.sql_type(mapping)),
                "length" | "year" | "month" => "integer".into(),
                _ => "keyword".into(),
            },
            Self::Case(branches, _) => branches
                .first()
                .map_or("keyword".into(), |(_, e)| e.sql_type(mapping)),
        }
    }

    /// The expression's value over one document's `_source`; null when an input is missing.
    pub(crate) fn eval(&self, src: &Value) -> Value {
        match self {
            Self::Field(f) => crate::result::dig(src, f).cloned().unwrap_or(Value::Null),
            Self::Lit(l) => lit_value(l),
            Self::Bin(op, a, b) => arith(*op, &a.eval(src), &b.eval(src)),
            Self::Case(branches, other) => {
                for (c, e) in branches {
                    if pred_matches(c, src) {
                        return e.eval(src);
                    }
                }
                other.as_ref().map_or(Value::Null, |e| e.eval(src))
            }
            Self::Call(name, args) => {
                let v: Vec<Value> = args.iter().map(|a| a.eval(src)).collect();
                call(name, &v)
            }
        }
    }
}

fn pred_fields(p: &Pred, out: &mut Vec<String>) {
    match p {
        Pred::And(v) | Pred::Or(v) => v.iter().for_each(|x| pred_fields(x, out)),
        Pred::Not(x) => pred_fields(x, out),
        Pred::Cmp(f, ..) | Pred::IsNull(f, _) if !out.contains(f) => out.push(f.clone()),
        _ => {}
    }
}

/// Whether a condition is one the in-memory evaluator can judge.
pub(crate) fn pred_supported(p: &Pred) -> bool {
    match p {
        Pred::And(v) | Pred::Or(v) => v.iter().all(pred_supported),
        Pred::Not(x) => pred_supported(x),
        Pred::Cmp(..) | Pred::IsNull(..) => true,
        _ => false,
    }
}

fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Str(s) => json!(s),
        Lit::Int(n) => json!(n),
        Lit::Float(f) => json!(f),
        Lit::Bool(b) => json!(b),
        Lit::Null => Value::Null,
    }
}

fn pred_matches(p: &Pred, src: &Value) -> bool {
    match p {
        Pred::And(v) => v.iter().all(|x| pred_matches(x, src)),
        Pred::Or(v) => v.iter().any(|x| pred_matches(x, src)),
        Pred::Not(x) => !pred_matches(x, src),
        Pred::IsNull(f, not) => {
            let null = crate::result::dig(src, f).is_none_or(Value::is_null);
            null != *not
        }
        Pred::Cmp(f, op, l) => {
            let Some(v) = crate::result::dig(src, f).filter(|v| !v.is_null()) else {
                return false;
            };
            compare(v, &lit_value(l)).is_some_and(|o| match op {
                CmpOp::Eq => o.is_eq(),
                CmpOp::Ne => o.is_ne(),
                CmpOp::Lt => o.is_lt(),
                CmpOp::Le => o.is_le(),
                CmpOp::Gt => o.is_gt(),
                CmpOp::Ge => o.is_ge(),
            })
        }
        _ => false,
    }
}

fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => a.as_f64()?.partial_cmp(&b.as_f64()?),
    }
}

fn arith(op: Arith, a: &Value, b: &Value) -> Value {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        let r = match op {
            Arith::Add => x.checked_add(y),
            Arith::Sub => x.checked_sub(y),
            Arith::Mul => x.checked_mul(y),
            Arith::Div => x.checked_div(y),
            Arith::Rem => x.checked_rem(y),
        };
        return r.map_or(Value::Null, |n| json!(n));
    }
    let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else {
        return Value::Null;
    };
    let r = match op {
        Arith::Add => x + y,
        Arith::Sub => x - y,
        Arith::Mul => x * y,
        Arith::Div if y == 0.0 => return Value::Null,
        Arith::Div => x / y,
        Arith::Rem if y == 0.0 => return Value::Null,
        Arith::Rem => x % y,
    };
    serde_json::Number::from_f64(r).map_or(Value::Null, Value::Number)
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// The year, month, day, hour, minute and second of an ISO date string.
fn parts(v: &Value) -> Option<[i64; 6]> {
    let s = v.as_str()?;
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    Some([
        num(0, 4)?,
        num(5, 7)?,
        num(8, 10)?,
        num(11, 13).unwrap_or(0),
        num(14, 16).unwrap_or(0),
        num(17, 19).unwrap_or(0),
    ])
}

fn date_format(v: &Value, fmt: &str) -> Value {
    let Some([y, mo, d, h, mi, s]) = parts(v) else {
        return Value::Null;
    };
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('Y') => out.push_str(&format!("{y:04}")),
            Some('m') => out.push_str(&format!("{mo:02}")),
            Some('d') => out.push_str(&format!("{d:02}")),
            Some('H') => out.push_str(&format!("{h:02}")),
            Some('i') => out.push_str(&format!("{mi:02}")),
            Some('s') => out.push_str(&format!("{s:02}")),
            Some('%') => out.push('%'),
            _ => return Value::Null,
        }
    }
    json!(out)
}

fn call(name: &str, v: &[Value]) -> Value {
    let one = v.first().unwrap_or(&Value::Null);
    match name {
        "abs" => match (one.as_i64(), one.as_f64()) {
            (Some(n), _) => n.checked_abs().map_or(Value::Null, |n| json!(n)),
            (_, Some(f)) => json!(f.abs()),
            _ => Value::Null,
        },
        "upper" => one
            .as_str()
            .map_or(Value::Null, |s| json!(s.to_uppercase())),
        "lower" => one
            .as_str()
            .map_or(Value::Null, |s| json!(s.to_lowercase())),
        "length" => one
            .as_str()
            .map_or(Value::Null, |s| json!(s.chars().count())),
        "concat" => {
            let parts: Option<Vec<String>> = v.iter().map(text).collect();
            parts.map_or(Value::Null, |p| json!(p.concat()))
        }
        "ifnull" if one.is_null() => v.get(1).cloned().unwrap_or(Value::Null),
        "ifnull" => one.clone(),
        "year" => parts(one).map_or(Value::Null, |p| json!(p[0])),
        "month" => parts(one).map_or(Value::Null, |p| json!(p[1])),
        "date_format" => match v.get(1).and_then(Value::as_str) {
            Some(f) => date_format(one, f),
            None => Value::Null,
        },
        _ => Value::Null,
    }
}
