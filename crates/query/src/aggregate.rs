//! Aggregates and `GROUP BY`: lowering to `aggregations` with one paged `composite`, and
//! the rows the buckets give.

use serde_json::{Map, Value, json};

use crate::ast::{Agg, CmpOp, Item, Lit, Select};
use crate::mapping::Field;
use crate::plan::{Column, Plan, Shape, Source, exact_name, lookup, query};
use crate::{Error, Mapping};

/// The name of the composite aggregation, as the plugin names it.
pub(crate) const COMPOSITE: &str = "composite_buckets";
/// Buckets asked for in one page of the composite.
const PAGE: usize = 1000;

fn numeric(f: &Field) -> bool {
    matches!(
        f.es_type.as_str(),
        "byte" | "short" | "integer" | "long" | "float" | "half_float" | "double" | "scaled_float"
    )
}

/// The aggregation body and result type of one aggregate.
fn lower(agg: &Agg, mapping: &Mapping) -> Result<(Value, String), Error> {
    let Some(field) = &agg.field else {
        return Ok((
            json!({"value_count": {"field": "_index"}}),
            "long".to_owned(),
        ));
    };
    let f = lookup(mapping, field)?;
    if agg.func == "count" {
        return Ok((json!({"value_count": {"field": field}}), "long".to_owned()));
    }
    if !numeric(f) {
        return Err(Error::semantic(format!(
            "{} function expected a number, but the field [{field}] is {}",
            agg.func,
            f.sql_type()
        )));
    }
    let ty = if agg.func == "avg" {
        "double"
    } else {
        f.sql_type()
    };
    Ok((json!({agg.func.clone(): {"field": field}}), ty.to_owned()))
}

pub(crate) fn plan(sel: &Select, mapping: &Mapping) -> Result<Plan, Error> {
    let mut group: Vec<String> = sel.group.clone();
    let mut columns = Vec::new();
    let mut aggs = Map::new();
    for item in &sel.items {
        match item {
            Item::Star => {
                return Err(Error::unsupported(
                    "* cannot be combined with aggregates or GROUP BY",
                ));
            }
            Item::Field(name, alias) => {
                if sel.distinct && !group.contains(name) {
                    group.push(name.clone());
                }
                if !group.contains(name) {
                    return Err(Error::semantic(format!(
                        "the field [{name}] must be in GROUP BY or inside an aggregate"
                    )));
                }
                let f = lookup(mapping, name)?;
                columns.push(Column {
                    name: name.clone(),
                    alias: alias.clone(),
                    ty: f.sql_type().to_owned(),
                    source: Source::Group(name.clone()),
                });
            }
            Item::Agg(agg, alias) => {
                let (body, ty) = lower(agg, mapping)?;
                let name = agg.name();
                aggs.insert(name.clone(), body);
                columns.push(Column {
                    name: name.clone(),
                    alias: alias.clone(),
                    ty,
                    source: Source::Agg(name),
                });
            }
        }
    }
    let mut body = Map::new();
    body.insert("from".into(), json!(0));
    body.insert("size".into(), json!(0));
    if let Some(filter) = &sel.filter {
        body.insert("query".into(), query(filter, mapping)?);
    }
    if group.is_empty() {
        if !sel.order.is_empty() {
            return Err(Error::unsupported(
                "ORDER BY needs GROUP BY next to aggregates",
            ));
        }
        body.insert("aggregations".into(), Value::Object(aggs));
    } else {
        let mut sources = Vec::new();
        for g in &group {
            let f = lookup(mapping, g)?;
            let name = exact_name(g, f, "group on")?;
            let desc = sel
                .order
                .iter()
                .find(|o| &o.field == g)
                .is_some_and(|o| o.desc);
            sources.push(json!({g.clone(): {"terms": {
                "field": name, "missing_bucket": true, "missing_order": "first",
                "order": if desc { "desc" } else { "asc" }}}}));
        }
        if let Some(o) = sel.order.iter().find(|o| !group.contains(&o.field)) {
            return Err(Error::unsupported(format!(
                "ORDER BY [{}] is not a GROUP BY field; ordering by an aggregate is a later slice",
                o.field
            )));
        }
        let mut comp = json!({"composite": {"size": PAGE, "sources": sources}});
        if !aggs.is_empty() {
            comp["aggregations"] = Value::Object(aggs);
        }
        body.insert("aggregations".into(), json!({COMPOSITE: comp}));
    }
    let having = match &sel.having {
        None => None,
        Some(h) => {
            let hit = columns.iter().find(|c| {
                matches!(c.source, Source::Agg(_))
                    && (c.name == h.target || c.alias.as_deref() == Some(&h.target))
            });
            let Some(c) = hit else {
                return Err(Error::semantic(format!(
                    "HAVING [{}] must name an aggregate of the select list",
                    h.target
                )));
            };
            Some((c.name.clone(), h.op, h.lit.clone()))
        }
    };
    Ok(Plan {
        body: Value::Object(body),
        columns,
        shape: Shape::Aggs {
            grouped: !group.is_empty(),
            having,
        },
    })
}

/// A number the way the column's type prints it: whole numbers as integers.
fn typed(ty: &str, v: Option<&Value>) -> Value {
    let Some(Value::Number(n)) = v else {
        return Value::Null;
    };
    match n.as_f64() {
        #[allow(clippy::cast_possible_truncation)]
        Some(f) if matches!(ty, "long" | "integer" | "short" | "byte") => json!(f as i64),
        _ => Value::Number(n.clone()),
    }
}

fn passes(v: &Value, op: CmpOp, lit: &Lit) -> bool {
    let (Some(a), Lit::Int(_) | Lit::Float(_)) = (v.as_f64(), lit) else {
        return false;
    };
    let b = match lit {
        #[allow(clippy::cast_precision_loss)]
        Lit::Int(n) => *n as f64,
        Lit::Float(f) => *f,
        _ => return false,
    };
    match op {
        CmpOp::Eq => a == b,
        CmpOp::Ne => a != b,
        CmpOp::Lt => a < b,
        CmpOp::Le => a <= b,
        CmpOp::Gt => a > b,
        CmpOp::Ge => a >= b,
    }
}

pub(crate) fn rows(plan: &Plan, response: &Value) -> Result<Vec<Vec<Value>>, Error> {
    let Shape::Aggs { grouped, having } = &plan.shape else {
        return Ok(Vec::new());
    };
    let aggs = response
        .get("aggregations")
        .ok_or_else(|| Error::unsupported("the search answer has no aggregations"))?;
    let empty = Vec::new();
    let buckets: Vec<&Value> = if *grouped {
        aggs.get(COMPOSITE)
            .and_then(|c| c.get("buckets"))
            .and_then(Value::as_array)
            .unwrap_or(&empty)
            .iter()
            .collect()
    } else {
        vec![aggs]
    };
    // ALLOC: one row a bucket; a page holds at most `PAGE` buckets.
    let mut out = Vec::with_capacity(buckets.len());
    for b in buckets {
        let row: Vec<Value> = plan
            .columns
            .iter()
            .map(|c| match &c.source {
                Source::Group(g) => b
                    .get("key")
                    .and_then(|k| k.get(g))
                    .cloned()
                    .unwrap_or(Value::Null),
                Source::Agg(a) => typed(&c.ty, b.get(a).and_then(|x| x.get("value"))),
                _ => Value::Null,
            })
            .collect();
        if let Some((name, op, lit)) = having {
            let at = plan.columns.iter().position(|c| &c.name == name);
            if !at
                .and_then(|i| row.get(i))
                .is_some_and(|v| passes(v, *op, lit))
            {
                continue;
            }
        }
        out.push(row);
    }
    Ok(out)
}
