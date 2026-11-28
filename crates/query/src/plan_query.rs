//! Lowers conditions to the query part of a search body.

use serde_json::{Map, Number, Value, json};

use crate::ast::{CmpOp, Lit, Pred, Relevance};
use crate::plan::{exact_name, lookup};
use crate::{Error, Field, Mapping};

fn is_rel(p: &Pred) -> bool {
    matches!(p, Pred::Rel(_))
}

pub(crate) fn query(p: &Pred, mapping: &Mapping) -> Result<Value, Error> {
    let conjuncts: Vec<&Pred> = match p {
        Pred::And(parts) => parts.iter().collect(),
        other => vec![other],
    };
    let mut must = Vec::new();
    let mut filter = Vec::new();
    for c in conjuncts {
        let q = node(c, mapping)?;
        if is_rel(c) {
            must.push(q)
        } else {
            filter.push(q)
        }
    }
    let mut b = Map::new();
    if !must.is_empty() {
        b.insert("must".into(), Value::Array(must));
    }
    if !filter.is_empty() {
        b.insert("filter".into(), Value::Array(filter));
    }
    Ok(json!({"bool": b}))
}

/// The plugin's name for a field's type in a comparison, when it has one a literal can clash with.
fn expr_type(f: &Field) -> Option<&'static str> {
    Some(match f.es_type.as_str() {
        "byte" => "BYTE",
        "short" => "SHORT",
        "integer" => "INTEGER",
        "long" => "LONG",
        "float" | "half_float" => "FLOAT",
        "double" | "scaled_float" => "DOUBLE",
        "keyword" | "text" => "STRING",
        _ => return None,
    })
}

/// A string against a number, or a number against a string, has no comparison function.
fn check_types(f: &Field, op: CmpOp, l: &Lit) -> Result<(), Error> {
    let Some(left) = expr_type(f) else {
        return Ok(());
    };
    let right = match l {
        Lit::Str(_) => "STRING",
        Lit::Int(_) => "INTEGER",
        Lit::Float(_) => "DOUBLE",
        _ => return Ok(()),
    };
    if (left == "STRING") == (right == "STRING") {
        return Ok(());
    }
    let func = match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    };
    Err(Error::type_mismatch(func, left, right))
}

/// A literal against a field: a date field takes the plugin's `yyyy-MM-dd[ HH:mm:ss[.fff]]`
/// strings as epoch milliseconds, as the plugin sends them.
fn lit_for(f: &Field, l: &Lit) -> Value {
    match l {
        Lit::Str(s) if matches!(f.es_type.as_str(), "date" | "date_nanos") => {
            crate::dates::millis(s).map_or_else(|| lit(l), |ms| json!(ms))
        }
        _ => lit(l),
    }
}

fn lit(l: &Lit) -> Value {
    match l {
        Lit::Str(s) => Value::String(s.clone()),
        Lit::Int(n) => json!(n),
        Lit::Float(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Null => Value::Null,
    }
}

/// A value that is present and not `q`: the plugin's `<>` and `NOT LIKE` skip a missing value.
fn present_and_not(field: &str, q: Value) -> Value {
    json!({"bool": {"filter": [{"exists": {"field": field}}], "must_not": [q]}})
}

fn not(q: Value) -> Value {
    json!({"bool": {"must_not": [q]}})
}

fn node(p: &Pred, m: &Mapping) -> Result<Value, Error> {
    Ok(match p {
        Pred::And(parts) => {
            let qs = parts
                .iter()
                .map(|x| node(x, m))
                .collect::<Result<Vec<_>, _>>()?;
            json!({"bool": {"filter": qs}})
        }
        Pred::Or(parts) => {
            let qs = parts
                .iter()
                .map(|x| node(x, m))
                .collect::<Result<Vec<_>, _>>()?;
            json!({"bool": {"should": qs, "minimum_should_match": 1}})
        }
        Pred::Not(inner) => not(node(inner, m)?),
        Pred::Cmp(field, op, l) => {
            let f = lookup(m, field)?;
            let name = exact_name(field, f, "compare")?;
            if matches!(l, Lit::Null) {
                return Err(Error::unsupported("compare with NULL using IS NULL"));
            }
            check_types(f, *op, l)?;
            let v = lit_for(f, l);
            match op {
                CmpOp::Eq => json!({"term": {name: v}}),
                // The plugin evaluates `<>` as a script that is false for a missing value.
                CmpOp::Ne => present_and_not(field, json!({"term": {name: v}})),
                CmpOp::Lt => json!({"range": {name: {"lt": v}}}),
                CmpOp::Le => json!({"range": {name: {"lte": v}}}),
                CmpOp::Gt => json!({"range": {name: {"gt": v}}}),
                CmpOp::Ge => json!({"range": {name: {"gte": v}}}),
            }
        }
        Pred::Between(field, lo, hi, neg) => {
            let f = lookup(m, field)?;
            let name = exact_name(field, f, "compare")?;
            let q = json!({"range": {name: {"gte": lit_for(f, lo), "lte": lit_for(f, hi)}}});
            if *neg { not(q) } else { q }
        }
        Pred::In(field, vals, neg) => {
            let f = lookup(m, field)?;
            let name = exact_name(field, f, "compare")?;
            let q =
                json!({"terms": {name: vals.iter().map(|v| lit_for(f, v)).collect::<Vec<_>>()}});
            if *neg { not(q) } else { q }
        }
        Pred::IsNull(field, is_not) => {
            lookup(m, field)?;
            let q = json!({"exists": {"field": field}});
            if *is_not { q } else { not(q) }
        }
        Pred::Like(field, pat, neg) => {
            let name = exact_name(field, lookup(m, field)?, "match with LIKE")?;
            let wild: String = pat
                .chars()
                .map(|c| match c {
                    '%' => '*',
                    '_' => '?',
                    o => o,
                })
                .collect();
            let q = json!({"wildcard": {name: {"wildcard": wild, "case_insensitive": true}}});
            if *neg { present_and_not(field, q) } else { q }
        }
        Pred::Rel(r) => relevance(r, m)?,
    })
}

fn relevance(r: &Relevance, m: &Mapping) -> Result<Value, Error> {
    for (f, _) in &r.fields {
        lookup(m, f)?;
    }
    let mut opts = Map::new();
    for (k, v) in &r.options {
        opts.insert(k.clone(), lit(v));
    }
    if matches!(
        r.func.as_str(),
        "multi_match" | "query_string" | "simple_query_string"
    ) {
        let fields: Map<String, Value> = r
            .fields
            .iter()
            .map(|(n, b)| (n.clone(), json!(b)))
            .collect();
        let mut body = Map::new();
        body.insert("query".into(), Value::String(r.query.clone()));
        body.insert("fields".into(), Value::Object(fields));
        body.extend(opts);
        return Ok(json!({r.func.clone(): body}));
    }
    let mut body = Map::new();
    body.insert("query".into(), Value::String(r.query.clone()));
    body.extend(opts);
    let field = r.fields.first().map_or("", |f| f.0.as_str());
    Ok(json!({r.func.clone(): {field: body}}))
}
