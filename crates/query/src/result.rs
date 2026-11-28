//! Rows out of a search answer and the three text formats.

use serde_json::{Map, Value, json};

use crate::plan::Source;
use crate::{Error, Plan};

/// A result format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The default: `schema`, `datarows`, `total`, `size`, `status`.
    Jdbc,
    /// Comma separated, with a header row.
    Csv,
    /// Pipe separated, no header, no quoting.
    Raw,
}

pub(crate) fn dig<'a>(src: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(v) = src.get(path) {
        return Some(v);
    }
    let mut cur = src;
    for step in path.split('.') {
        cur = cur.get(step)?;
    }
    Some(cur)
}

/// A date the way the plugin prints a timestamp: `yyyy-MM-dd HH:mm:ss`, with a fraction
/// only when there is one. Anything that is not an ISO instant is left as it is.
fn timestamp(v: Value) -> Value {
    let Value::String(s) = &v else { return v };
    let Some((date, time)) = s.split_once('T') else {
        return v;
    };
    let time = time.strip_suffix('Z').unwrap_or(time);
    let time = time.strip_suffix(".000").unwrap_or(time);
    if date.len() != 10 || time.contains(['+', 'Z']) {
        return v;
    }
    // ALLOC: one cell, bounded by the source field's length.
    Value::String(format!("{date} {time}"))
}

/// Reads one row a hit gives, for each column of the plan.
///
/// # Errors
///
/// An answer with no `hits.hits` array.
pub fn rows(plan: &Plan, response: &Value) -> Result<Vec<Vec<Value>>, Error> {
    if matches!(plan.shape, crate::plan::Shape::Aggs { .. }) {
        return crate::aggregate::rows(plan, response);
    }
    if matches!(plan.shape, crate::plan::Shape::Const) {
        let row = plan
            .columns()
            .iter()
            .map(|c| match &c.source {
                Source::Expr(e) => e.eval(&Value::Null),
                _ => Value::Null,
            })
            .collect();
        return Ok(vec![row]);
    }
    let hits = response
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::unsupported("the search answer has no hits"))?;
    // ALLOC: one row a hit; the page is bounded by the size limit the plan carries.
    let mut out = Vec::with_capacity(hits.len());
    for hit in hits {
        let src = hit.get("_source").unwrap_or(&Value::Null);
        let row = plan
            .columns()
            .iter()
            .map(|c| match &c.source {
                Source::Field(p) => {
                    let v = dig(src, p).cloned().unwrap_or(Value::Null);
                    match v {
                        // The plugin prints an empty array as null.
                        Value::Array(a) if a.is_empty() => Value::Null,
                        v if c.sql_type() == "timestamp" => timestamp(v),
                        v => v,
                    }
                }
                Source::Id => hit.get("_id").cloned().unwrap_or(Value::Null),
                Source::Index => hit.get("_index").cloned().unwrap_or(Value::Null),
                Source::Score => hit.get("_score").cloned().unwrap_or(Value::Null),
                Source::Expr(e) => e.eval(src),
                Source::Group(_) | Source::Agg(_) => Value::Null,
            })
            .collect();
        out.push(row);
    }
    Ok(out)
}

/// A byte-bounded text buffer: the first write past the bound is the error.
pub(crate) struct Out {
    buf: String,
    max: usize,
}

impl Out {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            buf: String::new(),
            max,
        }
    }

    pub(crate) fn push(&mut self, s: &str) -> Result<(), Error> {
        if self.buf.len().saturating_add(s.len()) > self.max {
            return Err(Error::too_large(self.max));
        }
        self.buf.push_str(s);
        Ok(())
    }

    pub(crate) fn finish(self) -> String {
        self.buf
    }
}

/// Writes rows in `format`, refusing a result past `max_bytes` rather than truncating it.
///
/// # Errors
///
/// A result past the byte bound.
pub fn write(
    format: Format,
    plan: &Plan,
    rows: &[Vec<Value>],
    max_bytes: usize,
) -> Result<String, Error> {
    let mut out = Out::new(max_bytes);
    match format {
        Format::Jdbc => {
            let schema: Vec<Value> = plan
                .columns()
                .iter()
                .map(|c| {
                    let mut m = Map::new();
                    m.insert("name".into(), json!(c.name()));
                    if let Some(a) = c.alias() {
                        m.insert("alias".into(), json!(a));
                    }
                    m.insert("type".into(), json!(c.sql_type()));
                    Value::Object(m)
                })
                .collect();
            jdbc(&mut out, schema, rows, !plan.ppl)?;
        }
        Format::Csv => {
            let header: Vec<String> = plan
                .columns()
                .iter()
                .map(|c| csv_cell(&Value::String(c.alias().unwrap_or(c.name()).to_owned())))
                .collect();
            out.push(&header.join(","))?;
            out.push("\n")?;
            for r in rows {
                let cells: Vec<String> = r.iter().map(csv_cell).collect();
                out.push(&cells.join(","))?;
                out.push("\n")?;
            }
        }
        Format::Raw => {
            for r in rows {
                let cells: Vec<String> = r.iter().map(plain).collect();
                out.push(&cells.join("|"))?;
                out.push("\n")?;
            }
        }
    }
    Ok(out.finish())
}

/// The `jdbc` form of `schema` and `rows`, written row by row against the bound.
pub(crate) fn jdbc(
    out: &mut Out,
    schema: Vec<Value>,
    rows: &[Vec<Value>],
    status: bool,
) -> Result<(), Error> {
    out.push("{\"schema\":")?;
    out.push(&Value::Array(schema).to_string())?;
    out.push(",\"datarows\":[")?;
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            out.push(",")?;
        }
        out.push(&serde_json::to_string(r).unwrap_or_default())?;
    }
    let tail = if status { ",\"status\":200" } else { "" };
    out.push(&format!(
        "],\"total\":{n},\"size\":{n}{tail}}}",
        n = rows.len()
    ))
}

fn plain(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn csv_cell(v: &Value) -> String {
    let mut s = plain(v);
    let mut quote = false;
    if matches!(v, Value::String(_)) && s.starts_with(['=', '+', '-', '@']) {
        s.insert(0, '\'');
        quote = true;
    }
    if quote || s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}
