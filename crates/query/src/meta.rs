//! `SHOW TABLES` and `DESCRIBE TABLES`, in the plugin's JDBC column sets.

use serde_json::{Value, json};

use crate::result::{Out, jdbc};
use crate::{Error, Mapping};

/// Whether `text` matches a SQL `LIKE` pattern (`%` any run, `_` one character).
#[must_use]
pub fn like(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '%' {
            star = Some((pi, ti));
            pi = pi.saturating_add(1);
        } else if pi < p.len() && (p[pi] == '_' || p[pi] == t[ti]) {
            pi = pi.saturating_add(1);
            ti = ti.saturating_add(1);
        } else if let Some((sp, st)) = star {
            pi = sp.saturating_add(1);
            ti = st.saturating_add(1);
            star = Some((sp, ti));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '%')
}

/// The text of a `LIKE` pattern before its first wildcard: every name the pattern matches
/// begins with it, so it is the prefix a listing needs to read.
#[must_use]
pub fn prefix_of(pattern: &str) -> &str {
    pattern
        .find(['%', '_'])
        .map_or(pattern, |at| pattern.get(..at).unwrap_or_default())
}

const SHOW: [(&str, &str); 10] = [
    ("TABLE_CAT", "keyword"),
    ("TABLE_SCHEM", "keyword"),
    ("TABLE_NAME", "keyword"),
    ("TABLE_TYPE", "keyword"),
    ("REMARKS", "keyword"),
    ("TYPE_CAT", "keyword"),
    ("TYPE_SCHEM", "keyword"),
    ("TYPE_NAME", "keyword"),
    ("SELF_REFERENCING_COL_NAME", "keyword"),
    ("REF_GENERATION", "keyword"),
];

const DESCRIBE: [(&str, &str); 24] = [
    ("TABLE_CAT", "keyword"),
    ("TABLE_SCHEM", "keyword"),
    ("TABLE_NAME", "keyword"),
    ("COLUMN_NAME", "keyword"),
    ("DATA_TYPE", "integer"),
    ("TYPE_NAME", "keyword"),
    ("COLUMN_SIZE", "integer"),
    ("BUFFER_LENGTH", "integer"),
    ("DECIMAL_DIGITS", "integer"),
    ("NUM_PREC_RADIX", "integer"),
    ("NULLABLE", "integer"),
    ("REMARKS", "keyword"),
    ("COLUMN_DEF", "keyword"),
    ("SQL_DATA_TYPE", "integer"),
    ("SQL_DATETIME_SUB", "integer"),
    ("CHAR_OCTET_LENGTH", "integer"),
    ("ORDINAL_POSITION", "integer"),
    ("IS_NULLABLE", "keyword"),
    ("SCOPE_CATALOG", "keyword"),
    ("SCOPE_SCHEMA", "keyword"),
    ("SCOPE_TABLE", "keyword"),
    ("SOURCE_DATA_TYPE", "short"),
    ("IS_AUTOINCREMENT", "keyword"),
    ("IS_GENERATEDCOLUMN", "keyword"),
];

fn schema(cols: &[(&str, &str)]) -> Vec<Value> {
    cols.iter()
        .map(|(n, t)| json!({"name": n, "type": t}))
        .collect()
}

/// The `SHOW TABLES` answer for the already filtered `names`, at most `limit` rows.
///
/// # Errors
///
/// Never in practice; the writer's bound is the limit's.
pub fn show_tables(cluster: &str, names: &[String], limit: usize) -> Result<String, Error> {
    let rows: Vec<Vec<Value>> = names
        .iter()
        .take(limit)
        .map(|n| {
            vec![
                json!(cluster),
                Value::Null,
                json!(n),
                json!("BASE TABLE"),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ]
        })
        .collect();
    let mut out = Out::new(usize::MAX);
    jdbc(&mut out, schema(&SHOW), &rows, true)?;
    Ok(out.finish())
}

/// The `DESCRIBE TABLES` answer for one table's `mapping`, keeping the columns that match `column_like`.
///
/// # Errors
///
/// Never in practice; the writer's bound is the limit's.
pub fn describe(
    cluster: &str,
    table: &str,
    mapping: &Mapping,
    column_like: Option<&str>,
    limit: usize,
) -> Result<String, Error> {
    describe_many(cluster, &[(table, mapping)], column_like, limit)
}

/// The `DESCRIBE TABLES` answer for several tables, one after another in the order given, at
/// most `limit` rows in all; each table numbers its own columns from 1.
///
/// # Errors
///
/// Never in practice; the writer's bound is the limit's.
pub fn describe_many(
    cluster: &str,
    tables: &[(&str, &Mapping)],
    column_like: Option<&str>,
    limit: usize,
) -> Result<String, Error> {
    let mut rows = Vec::new();
    for (table, mapping) in tables {
        for (at, (name, ty)) in mapping.describe_columns().iter().enumerate() {
            if column_like.is_some_and(|p| !like(p, name)) {
                continue;
            }
            if rows.len() >= limit {
                break;
            }
            let ordinal = i64::try_from(at).unwrap_or(i64::MAX).saturating_add(1);
            rows.push(column_row(cluster, table, name, ty, ordinal));
        }
    }
    let mut out = Out::new(usize::MAX);
    jdbc(&mut out, schema(&DESCRIBE), &rows, true)?;
    Ok(out.finish())
}

/// One `DESCRIBE` row, as the plugin answers it: the type is the mapping's own name, and the
/// sizes and the JDBC code are null.
fn column_row(cluster: &str, table: &str, name: &str, ty: &str, ordinal: i64) -> Vec<Value> {
    vec![
        json!(cluster),
        Value::Null,
        json!(table),
        json!(name),
        Value::Null,
        json!(ty),
        Value::Null,
        Value::Null,
        Value::Null,
        json!(10),
        json!(2),
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        json!(ordinal),
        json!(""),
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
        json!("NO"),
        json!(""),
    ]
}
