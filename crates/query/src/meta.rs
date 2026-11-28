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
    jdbc(&mut out, schema(&SHOW), &rows)?;
    Ok(out.finish())
}

/// The JDBC type code the plugin gives a mapping type.
fn jdbc_code(sql_type: &str) -> i64 {
    match sql_type {
        "boolean" => 16,
        "byte" => -6,
        "short" => 5,
        "integer" => 4,
        "long" => -5,
        "float" => 7,
        "double" => 8,
        "timestamp" => 93,
        _ => 12,
    }
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
    let mut rows = Vec::new();
    let mut ordinal = 0i64;
    for (name, f) in mapping.fields() {
        if column_like.is_some_and(|p| !like(p, name)) {
            continue;
        }
        ordinal = ordinal.saturating_add(1);
        if rows.len() >= limit {
            break;
        }
        let ty = f.sql_type();
        rows.push(vec![
            json!(cluster),
            Value::Null,
            json!(table),
            json!(name),
            json!(jdbc_code(ty)),
            json!(ty.to_ascii_uppercase()),
            Value::Null,
            json!(0),
            Value::Null,
            json!(10),
            json!(2),
            Value::Null,
            Value::Null,
            json!(0),
            json!(0),
            Value::Null,
            json!(ordinal),
            json!(""),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
            json!("NO"),
            json!(""),
        ]);
    }
    let mut out = Out::new(usize::MAX);
    jdbc(&mut out, schema(&DESCRIBE), &rows)?;
    Ok(out.finish())
}
