//! Lowers a `SELECT` to a search body and the columns it answers.

use serde_json::{Map, Number, Value, json};

use crate::ast::{CmpOp, Item, Lit, Pred, Relevance, Select};
use crate::{Error, Field, Mapping};

/// Where a column's value comes from in a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    Field(String),
    Id,
    Index,
    Score,
    /// A `GROUP BY` key of a bucket.
    Group(String),
    /// An aggregate of a bucket or of the whole result.
    Agg(String),
}

/// What a plan's answer looks like.
#[derive(Debug, Clone)]
pub(crate) enum Shape {
    /// One row a hit.
    Hits,
    /// Rows from aggregations: whether a composite pages them, and the `HAVING` to apply.
    Aggs {
        grouped: bool,
        having: Option<(String, crate::ast::CmpOp, crate::ast::Lit)>,
    },
}

/// One output column.
#[derive(Debug, Clone)]
pub struct Column {
    pub(crate) name: String,
    pub(crate) alias: Option<String>,
    pub(crate) ty: String,
    pub(crate) source: Source,
}

impl Column {
    /// The field or metadata name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name the statement gave it with `AS`.
    #[must_use]
    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    /// The type name the plugin reports.
    #[must_use]
    pub fn sql_type(&self) -> &str {
        &self.ty
    }
}

/// A search body and the columns it answers.
#[derive(Debug, Clone)]
pub struct Plan {
    pub(crate) body: Value,
    pub(crate) columns: Vec<Column>,
    pub(crate) shape: Shape,
}

impl Plan {
    /// The search body.
    #[must_use]
    pub fn body(&self) -> &Value {
        &self.body
    }

    /// Whether the answer is one page of a composite aggregation, so the caller asks for the
    /// next page while [`Plan::next_page`] gives one.
    #[must_use]
    pub fn is_paged(&self) -> bool {
        matches!(self.shape, Shape::Aggs { grouped: true, .. })
    }

    /// The body that asks for the page after the one `response` answers, or `None` when it
    /// was the last.
    #[must_use]
    pub fn next_page(&self, response: &Value) -> Option<Value> {
        if !self.is_paged() {
            return None;
        }
        let after = response
            .get("aggregations")?
            .get(crate::aggregate::COMPOSITE)?
            .get("after_key")?;
        let mut body = self.body.clone();
        body.get_mut("aggregations")?
            .get_mut(crate::aggregate::COMPOSITE)?
            .get_mut("composite")?
            .as_object_mut()?
            .insert("after".into(), after.clone());
        Some(body)
    }

    /// How many buckets `response` holds (one for an ungrouped aggregate, none for hits).
    #[must_use]
    pub fn buckets(&self, response: &Value) -> usize {
        match self.shape {
            Shape::Aggs { grouped: true, .. } => response
                .get("aggregations")
                .and_then(|a| a.get(crate::aggregate::COMPOSITE))
                .and_then(|c| c.get("buckets"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            _ => 0,
        }
    }

    /// The columns, in select order.
    #[must_use]
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
}

fn meta(name: &str) -> Option<(Source, &'static str)> {
    match name {
        "_id" => Some((Source::Id, "keyword")),
        "_index" => Some((Source::Index, "keyword")),
        "_score" => Some((Source::Score, "float")),
        _ => None,
    }
}

pub(crate) fn lookup<'a>(mapping: &'a Mapping, name: &str) -> Result<&'a Field, Error> {
    mapping.field(name).ok_or_else(|| {
        // ALLOC: one message, bounded by the identifier limit.
        Error::semantic(format!(
            "can't resolve Symbol(namespace=FIELD_NAME, name={name}) in type env"
        ))
    })
}

/// Lowers `sel` over `mapping`; a result never has more than `size_limit` rows.
///
/// # Errors
///
/// A field the mapping lacks, or a construct with no search-body form.
pub fn plan(sel: &Select, mapping: &Mapping, size_limit: usize) -> Result<Plan, Error> {
    if sel.distinct || !sel.group.is_empty() || sel.items.iter().any(|i| matches!(i, Item::Agg(..)))
    {
        return crate::aggregate::plan(sel, mapping);
    }
    let mut columns = Vec::new();
    let mut includes: Vec<Value> = Vec::new();
    let mut score = false;
    for item in &sel.items {
        match item {
            Item::Star => {
                for (name, leaf) in mapping.star_columns() {
                    includes.push(Value::String(name.clone()));
                    columns.push(Column {
                        name: name.clone(),
                        alias: None,
                        ty: leaf.map_or("object", Field::sql_type).to_owned(),
                        source: Source::Field(name),
                    });
                }
            }
            Item::Agg(..) => unreachable!("aggregates take the aggregate path"),
            Item::Field(name, alias) => {
                if let Some((source, ty)) = meta(name) {
                    score |= source == Source::Score;
                    columns.push(Column {
                        name: name.clone(),
                        alias: alias.clone(),
                        ty: ty.to_owned(),
                        source,
                    });
                } else {
                    let f = lookup(mapping, name)?;
                    includes.push(Value::String(name.clone()));
                    columns.push(Column {
                        name: name.clone(),
                        alias: alias.clone(),
                        ty: f.sql_type().to_owned(),
                        source: Source::Field(name.clone()),
                    });
                }
            }
        }
    }
    let star_only = sel.items.iter().all(|i| matches!(i, Item::Star));
    let mut body = Map::new();
    let size = sel.limit.map_or(size_limit, |l| {
        usize::try_from(l).unwrap_or(usize::MAX).min(size_limit)
    });
    body.insert("from".into(), json!(sel.offset));
    body.insert("size".into(), json!(size));
    if let Some(filter) = &sel.filter {
        body.insert("query".into(), query(filter, mapping)?);
    }
    if !star_only {
        if includes.is_empty() {
            body.insert("_source".into(), Value::Bool(false));
        } else {
            body.insert(
                "_source".into(),
                json!({"includes": includes, "excludes": []}),
            );
        }
    }
    let mut sort = Vec::new();
    for o in &sel.order {
        if o.field == "_score" {
            score = true;
            sort.push(json!({"_score": {"order": order(o.desc)}}));
            continue;
        }
        let f = lookup(mapping, &o.field)?;
        let key = exact_name(&o.field, f, "sort on")?;
        sort.push(json!({key: {"order": order(o.desc), "missing": if o.nulls_first.unwrap_or(!o.desc) { "_first" } else { "_last" }}}));
    }
    if score {
        body.insert("track_scores".into(), Value::Bool(true));
    }
    if !sort.is_empty() {
        body.insert("sort".into(), Value::Array(sort));
    }
    Ok(Plan {
        body: Value::Object(body),
        columns,
        shape: Shape::Hits,
    })
}

fn order(desc: bool) -> &'static str {
    if desc { "desc" } else { "asc" }
}

/// The name a term, range, wildcard or sort runs on: a text field's keyword form.
pub(crate) fn exact_name(name: &str, f: &Field, what: &str) -> Result<String, Error> {
    if f.es_type == "text" {
        if f.keyword_sub {
            // ALLOC: one name, bounded by the identifier limit.
            return Ok(format!("{name}.keyword"));
        }
        if !matches!(what, "sort on" | "group on") {
            // The plugin runs the term on the analysed field itself.
            return Ok(name.to_owned());
        }
        return Err(Error::unsupported(format!(
            "cannot {what} the text field [{name}]: it has no keyword sub-field"
        )));
    }
    if matches!(f.es_type.as_str(), "object" | "nested") {
        return Err(Error::unsupported(format!(
            "cannot {what} the object field [{name}]"
        )));
    }
    Ok(name.to_owned())
}

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
