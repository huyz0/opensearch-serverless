//! Lowers a `SELECT` to a search body and the columns it answers.

use serde_json::{Map, Value, json};

use crate::ast::{Dialect, Item, Select};
use crate::plan_query::query;
use crate::{Error, Field, Mapping};

/// Where a column's value comes from in a hit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Source {
    Field(String),
    /// A scalar expression, evaluated over each hit's `_source`.
    Expr(crate::expr::Expr),
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
    /// One row of constants: a `SELECT` with no `FROM`, answered without a search.
    Const,
    /// Rows from aggregations: whether a composite pages them, and the `HAVING` to apply.
    Aggs {
        grouped: bool,
        having: Option<(String, crate::ast::CmpOp, crate::ast::Lit)>,
        /// Columns to order the bucket rows by when the order is not on group keys.
        sort: Vec<(usize, bool)>,
        offset: usize,
        /// The most rows kept after ordering.
        limit: usize,
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
    pub(crate) ppl: bool,
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

    /// The rows ordered, offset and cut as the statement says: the part of an aggregate
    /// statement that needs every page first.
    #[must_use]
    pub fn finish(&self, rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
        crate::aggregate::finish(&self.shape, rows)
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
    match sel.dialect {
        Dialect::Sql => plan_sql(sel, mapping, size_limit),
        Dialect::Ppl => plan_sql(sel, mapping, size_limit)
            .map(ppl_names)
            .map_err(Error::into_ppl),
    }
}

/// PPL's names for what the planner lowered: an alias is the column's name, `count(*)` is
/// `count()`, and the types are PPL's.
fn ppl_names(mut p: Plan) -> Plan {
    for c in &mut p.columns {
        c.name = match c.alias.take() {
            Some(a) => a,
            None if c.name == "count(*)" => "count()".to_owned(),
            None => c.name.clone(),
        };
        c.ty = match c.ty.as_str() {
            "keyword" | "text" => "string",
            "integer" => "int",
            "long" => "bigint",
            "short" => "smallint",
            "byte" => "tinyint",
            "object" | "nested" => "struct",
            other => other,
        }
        .to_owned();
    }
    p.ppl = true;
    p
}

/// `SELECT <constants>` with no `FROM`: one row, no search.
fn plan_const(sel: &Select) -> Result<Plan, Error> {
    let mut columns = Vec::new();
    for item in &sel.items {
        let Item::Expr(expr, alias, text) = item else {
            return Err(Error::unsupported(
                "a SELECT with no FROM takes expressions only",
            ));
        };
        columns.push(Column {
            name: text.clone(),
            alias: alias.clone(),
            ty: expr.sql_type(&Mapping::default()),
            source: Source::Expr(expr.clone()),
        });
    }
    Ok(Plan {
        body: json!({}),
        columns,
        shape: Shape::Const,
        ppl: false,
    })
}

fn plan_sql(sel: &Select, mapping: &Mapping, size_limit: usize) -> Result<Plan, Error> {
    if sel.from.is_empty() {
        return plan_const(sel);
    }
    if sel.distinct || !sel.group.is_empty() || sel.items.iter().any(|i| matches!(i, Item::Agg(..)))
    {
        return crate::aggregate::plan(sel, mapping, size_limit);
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
            Item::StarExcept(skip) => {
                for (name, leaf) in mapping.star_columns() {
                    if leaf.is_some() {
                        if !skip.contains(&name) {
                            includes.push(Value::String(name.clone()));
                            columns.push(Column {
                                name: name.clone(),
                                alias: None,
                                ty: leaf.map_or("object", Field::sql_type).to_owned(),
                                source: Source::Field(name),
                            });
                        }
                        continue;
                    }
                    // An object: its own column, then each leaf under it.
                    if !skip.contains(&name) {
                        columns.push(Column {
                            name: name.clone(),
                            alias: None,
                            ty: "object".to_owned(),
                            source: Source::Field(name.clone()),
                        });
                    }
                    let under = format!("{name}.");
                    for (path, f) in mapping.fields().filter(|(p, _)| p.starts_with(&under)) {
                        if !skip.iter().any(|s| s == path) {
                            includes.push(Value::String(path.to_owned()));
                            columns.push(Column {
                                name: path.to_owned(),
                                alias: None,
                                ty: f.sql_type().to_owned(),
                                source: Source::Field(path.to_owned()),
                            });
                        }
                    }
                }
            }
            Item::Agg(..) => unreachable!("aggregates take the aggregate path"),
            Item::Expr(expr, alias, text) => {
                let mut read = Vec::new();
                expr.fields(&mut read);
                for f in read {
                    lookup(mapping, &f)?;
                    let f = Value::String(f);
                    if !includes.contains(&f) {
                        includes.push(f);
                    }
                }
                columns.push(Column {
                    name: text.clone(),
                    alias: alias.clone(),
                    ty: expr.sql_type(mapping),
                    source: Source::Expr(expr.clone()),
                });
            }
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
        ppl: false,
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
