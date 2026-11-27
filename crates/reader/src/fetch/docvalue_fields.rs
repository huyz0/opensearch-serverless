//! `docvalue_fields`: each hit's values of the fields asked for, read from
//! their doc values.
//!
//! Ported from `FieldAndFormat.fromXContent`
//! (server/src/main/java/org/opensearch/search/fetch/subphase/FieldAndFormat.java),
//! the limit `SearchService.parseSource` checks, and
//! `DocValueFieldsFetchSubPhase.getProcessor` and `process`
//! (server/src/main/java/org/opensearch/search/fetch/subphase/DocValueFieldsFetchSubPhase.java),
//! at OpenSearch 3.10.0. A name or a `*` pattern is resolved over the
//! mapping; an unmapped name is skipped. Integer and long fields write their
//! values as numbers, dates in the default format, keywords as strings,
//! each in doc-values order. A field asked for twice writes its values
//! twice. The hit's `fields` object comes out in `java.util.HashMap` order,
//! as the hit's document fields do. A `format` other than
//! `use_field_mapping`, a metadata field other than `_seq_no` and `_primary_term`, and a pattern that could match one
//! are not served yet.

use lucene_util::half_float::sortable_short_to_half_float;
use lucene_util::numeric_utils::{sortable_int_to_float, sortable_long_to_double};
use std::io::{self, Write};

use lucene_search::directory_reader::SegmentReader;
use lucene_search::reader::doc_values::{get_sorted_numeric, get_sorted_set};
use oss_rest::write_json_str;

use crate::aggs::metric::date_text::write_date;
use crate::dsl::error::DslError;
use crate::dsl::queries::build::simple_match;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;
use crate::failure::Failure;
use crate::fetch::source_map::java_hash_order;
use crate::mapping::{FieldType, Mapping};
use crate::search::ShardError;

/// `index.max_docvalue_fields_search`'s default: the most entries a request
/// may list.
pub const MAX_DOCVALUE_FIELDS: usize = 100;

/// One entry of `docvalue_fields`: a field name or pattern, and its format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocValueField {
    /// The field, or a `*` pattern.
    pub field: String,
    /// `format`, when given.
    pub format: Option<String>,
}

const NAME: &str = "docvalues_field";

fn xcontent(p: &Parser<'_>, reason: &str) -> DslError {
    // ALLOC: error path; one message.
    DslError::xcontent(p.location(), &format!("[{NAME}] {reason}"), None)
}

/// The `docvalue_fields` array, the parser on its start.
pub fn parse_docvalue_fields(p: &mut Parser<'_>) -> Result<Vec<DocValueField>, DslError> {
    // ALLOC: one entry an element, bounded by the body's recording.
    let mut fields = Vec::new();
    loop {
        let field = match p.next_token() {
            Some(Kind::EndArray) | None => break,
            Some(Kind::String | Kind::Number | Kind::Bool) => DocValueField {
                // ALLOC: one name an entry, bounded by the body's recording.
                field: p.text().to_owned(),
                format: None,
            },
            Some(Kind::StartObject) => object(p)?,
            // `ObjectParser.parse` moves past a token that does not start
            // an object and refuses the next.
            Some(_) => {
                let next = p.next_token();
                return Err(xcontent(
                    p,
                    // ALLOC: error path; one message.
                    &format!(
                        "Expected START_OBJECT but was: {}",
                        next.map_or("null", Kind::name)
                    ),
                ));
            }
        };
        fields.push(field);
    }
    Ok(fields)
}

/// `FieldAndFormat.PARSER` on an object: `field`, required, and `format`.
fn object(p: &mut Parser<'_>) -> Result<DocValueField, DslError> {
    let mut field = None;
    let mut format = None;
    let mut current = "";
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                current = p.text();
                if !matches!(current, "field" | "format") {
                    return Err(xcontent(
                        p,
                        // ALLOC: error path; one message.
                        &format!(
                            "unknown field [{current}]{}",
                            suggest(current, &["field", "format"])
                        ),
                    ));
                }
            }
            // ALLOC: one name, bounded by the body's recording.
            Some(Kind::String) if current == "field" => field = Some(p.text().to_owned()),
            // ALLOC: one format, bounded by the body's recording.
            Some(Kind::String) => format = Some(p.text().to_owned()),
            Some(Kind::Null) if current == "format" => format = None,
            Some(k) => {
                return Err(xcontent(
                    p,
                    // ALLOC: error path; one message.
                    &format!("{current} doesn't support values of type: {}", k.name()),
                ));
            }
        }
    }
    let Some(field) = field else {
        return Err(DslError::illegal("Required [field]"));
    };
    Ok(DocValueField { field, format })
}

/// How a field's doc values are read and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// Sorted numeric longs, written as numbers.
    Long,
    /// Sorted numeric sortable longs, written as doubles.
    Double,
    /// Sorted numeric sortable ints, each widened and written as a double.
    Float,
    /// Sorted numeric sortable shorts, each widened and written as a double.
    HalfFloat,
    /// Sorted numeric epoch milliseconds, written as dates.
    Date,
    /// Sorted set terms, written as strings.
    Term,
    /// A script's number, written as a Java double.
    Script,
    /// Values read from the hit's `_source` (the `fields` API).
    Source,
    /// The feature values `ext.ltr_log` logged, written as `_ltrlog`.
    LtrLog,
}

/// A resolved field: its name, how it reads, and how many times the request
/// asked for it.
#[derive(Clone, Debug, PartialEq)]
pub struct HitField {
    /// The field.
    pub name: String,
    /// How its values read.
    pub kind: ValueKind,
    /// How many entries resolved to it.
    pub times: usize,
    /// The script, for a `script_fields` entry.
    pub script: Option<std::sync::Arc<lucene_engine::painless_score::ScriptFunction>>,
    /// Where the values sit in `_source`, for a `fields` entry.
    pub source: Option<crate::fetch::source_fields::SourceFetch>,
    /// The logged feature values, for `_ltrlog`.
    pub ltr: Option<std::sync::Arc<crate::search_ltr_log::LtrLogged>>,
}

fn bad_request(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

fn unsupported(what: &str) -> ShardError {
    // ALLOC: error path; one message.
    bad_request(format!("{what} is not supported by this node yet"))
}

/// `SearchService.parseSource`'s limit on how many entries a request lists.
pub fn check_count(fields: &[DocValueField]) -> Result<(), ShardError> {
    if fields.len() > MAX_DOCVALUE_FIELDS {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "Trying to retrieve too many docvalue_fields. Must be less than or equal to: \
             [{MAX_DOCVALUE_FIELDS}] but was [{}]. This limit can be set by changing the \
             [index.max_docvalue_fields_search] index level setting.",
            fields.len()
        )));
    }
    Ok(())
}

/// `DocValueFieldsFetchSubPhase.getProcessor`: each entry's fields over
/// `mapping`, failing as a field's `fielddataBuilder` fails; the fields in
/// the order the hit's `fields` object writes them.
pub fn resolve(fields: &[DocValueField], mapping: &Mapping) -> Result<Vec<HitField>, ShardError> {
    // ALLOC: one entry a distinct mapped field, at most MAX_FIELDS.
    let mut out: Vec<HitField> = Vec::new();
    for entry in fields {
        if entry
            .format
            .as_deref()
            .is_some_and(|f| f != "use_field_mapping")
        {
            return Err(unsupported("a [docvalue_fields] format"));
        }
        let name = entry.field.as_str();
        if name.starts_with(['_', '*']) && !is_sequence_field(name) {
            // ALLOC: error path; one message.
            return Err(unsupported(&format!(
                "[docvalue_fields] of [{name}], which can name a metadata field,"
            )));
        }
        // `simpleMatchToFullName` gives a pattern's fields as a `HashSet`.
        // ALLOC: one name a mapped field, at most MAX_FIELDS.
        let names: Vec<&str> = if name.contains('*') {
            mapping.names().filter(|n| simple_match(name, n)).collect()
        } else {
            let known = is_sequence_field(name) || mapping.get(name).is_some();
            // ALLOC: as above.
            known.then_some(name).into_iter().collect()
        };
        // ALLOC: one index a name, as above.
        let mut order = Vec::new();
        java_hash_order(&names, &mut order);
        for n in order.iter().filter_map(|&i| names.get(i).copied()) {
            let kind = kind_of(n, mapping)?;
            match out.iter_mut().find(|f| f.name == n) {
                Some(f) => f.times = f.times.saturating_add(1),
                None => out.push(HitField {
                    // ALLOC: one name a distinct field, as above.
                    name: n.to_owned(),
                    kind,
                    times: 1,
                    script: None,
                    source: None,
                    ltr: None,
                }),
            }
        }
    }
    // ALLOC: one name a resolved field, at most MAX_FIELDS.
    let names: Vec<&str> = out.iter().map(|f| f.name.as_str()).collect();
    // ALLOC: one index a resolved field, as above.
    let mut order = Vec::new();
    java_hash_order(&names, &mut order);
    // ALLOC: the resolved fields once more, in order, as above.
    Ok(order.iter().filter_map(|&i| out.get(i).cloned()).collect())
}

/// `_seq_no` and `_primary_term`: the metadata fields whose doc values a hit
/// may carry, as long fields.
fn is_sequence_field(name: &str) -> bool {
    matches!(name, "_seq_no" | "_primary_term")
}

/// How the mapped field `name` reads, or OpenSearch's refusal.
fn kind_of(name: &str, mapping: &Mapping) -> Result<ValueKind, ShardError> {
    if is_sequence_field(name) {
        return Ok(ValueKind::Long);
    }
    // Only mapped names are resolved.
    let Some(field) = mapping.get(name) else {
        return Ok(ValueKind::Term);
    };
    let (kind, type_name) = match &field.ty {
        FieldType::Text => {
            // ALLOC: error path; one message.
            return Err(bad_request(format!(
                "Text fields are not optimised for operations that require per-document field \
                 data like aggregations and sorting, so these operations are disabled by default. \
                 Please use a keyword field instead. Alternatively, set fielddata=true on [{name}] \
                 in order to load field data by uninverting the inverted index. Note that this can \
                 use significant memory."
            )));
        }
        // ALLOC: error path; one message.
        FieldType::Unsupported(what) => {
            return Err(unsupported(&format!(
                "[docvalue_fields] of the {what} [{name}]"
            )));
        }
        FieldType::Keyword => (ValueKind::Term, "keyword"),
        FieldType::Int { name: ty, .. } => (ValueKind::Long, *ty),
        FieldType::Long => (ValueKind::Long, "long"),
        FieldType::Double => (ValueKind::Double, "double"),
        FieldType::Float => (ValueKind::Float, "float"),
        FieldType::HalfFloat => (ValueKind::HalfFloat, "half_float"),
        FieldType::Date => (ValueKind::Date, "date"),
    };
    if !field.doc_values {
        // ALLOC: error path; one message.
        return Err(bad_request(format!(
            "Can't load fielddata on [{name}] because fielddata is unsupported on fields of type \
             [{type_name}]. Use doc values instead."
        )));
    }
    Ok(kind)
}

/// Writes `,"fields":{...}` for document `doc` of `segment`, or nothing when
/// none of `fields` has a value there.
pub fn write_fields(
    out: &mut dyn Write,
    fields: &[HitField],
    segment: &SegmentReader,
    doc: i32,
) -> io::Result<()> {
    let mut opened = false;
    for field in fields {
        let values = match field.kind {
            // Written from the `_source`, by `source_fields`.
            ValueKind::Source => continue,
            ValueKind::LtrLog => {
                let global = segment.doc_base.saturating_add(doc);
                if let Some(logged) = &field.ltr
                    && logged.logs.iter().any(|l| l.rows.contains_key(&global))
                {
                    out.write_all(if opened { b"," } else { b",\"fields\":{" })?;
                    opened = true;
                    crate::search_ltr_log::write(out, logged, global)?;
                }
                continue;
            }
            ValueKind::Long | ValueKind::Date => numbers(segment, &field.name, doc)?,
            ValueKind::Double | ValueKind::Float | ValueKind::HalfFloat => {
                decimals(segment, &field.name, doc, field.kind)?
            }
            ValueKind::Term => terms(segment, &field.name, doc)?,
            ValueKind::Script => crate::fetch::script_fields::values(field, segment, doc)?,
        };
        if values.is_empty() {
            continue;
        }
        out.write_all(if opened { b"," } else { b",\"fields\":{" })?;
        opened = true;
        write_json_str(out, &field.name)?;
        out.write_all(b":[")?;
        let repeated = (0..field.times).flat_map(|_| values.iter());
        for (n, value) in repeated.enumerate() {
            if n > 0 {
                out.write_all(b",")?;
            }
            match (field.kind, value) {
                (ValueKind::Long, Value::Long(n)) => write!(out, "{n}")?,
                (_, Value::Double(d)) => {
                    out.write_all(crate::template::java_double(*d).as_bytes())?;
                }
                (_, Value::Long(n)) => {
                    out.write_all(b"\"")?;
                    write_date(out, *n)?;
                    out.write_all(b"\"")?;
                }
                (_, Value::Term(t)) => write_json_str(out, &String::from_utf8_lossy(t))?,
            }
        }
        out.write_all(b"]")?;
    }
    if opened {
        out.write_all(b"}")?;
    }
    Ok(())
}

/// One doc value.
pub(crate) enum Value {
    Long(i64),
    Double(f64),
    Term(Vec<u8>),
}

pub(crate) fn numbers(segment: &SegmentReader, field: &str, doc: i32) -> io::Result<Vec<Value>> {
    let mut dv = get_sorted_numeric(segment, field).map_err(io::Error::other)?;
    if !dv.advance_exact(doc).map_err(io::Error::other)? {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    let count = usize::try_from(dv.doc_value_count()).unwrap_or(0);
    // ALLOC: one value a value of the field in this document.
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(Value::Long(dv.next_value().map_err(io::Error::other)?));
    }
    Ok(values)
}

/// A `double` or `float` field's values: each sortable number read as the
/// double `SortedNumericDoubleValues` gives, a float widened.
fn decimals(
    segment: &SegmentReader,
    field: &str,
    doc: i32,
    kind: ValueKind,
) -> io::Result<Vec<Value>> {
    let mut values = numbers(segment, field, doc)?;
    for v in &mut values {
        if let Value::Long(n) = *v {
            *v = Value::Double(match kind {
                ValueKind::Float => f64::from(sortable_int_to_float(i32::try_from(n).unwrap_or(0))),
                ValueKind::HalfFloat => {
                    f64::from(sortable_short_to_half_float(i16::try_from(n).unwrap_or(0)))
                }
                _ => sortable_long_to_double(n),
            });
        }
    }
    Ok(values)
}

pub(crate) fn terms(segment: &SegmentReader, field: &str, doc: i32) -> io::Result<Vec<Value>> {
    let mut dv = get_sorted_set(segment, field).map_err(io::Error::other)?;
    if !dv.advance_exact(doc).map_err(io::Error::other)? {
        // ALLOC: empty.
        return Ok(Vec::new());
    }
    let count = usize::try_from(dv.doc_value_count()).unwrap_or(0);
    // ALLOC: one ordinal a value of the field in this document.
    let mut ords = Vec::with_capacity(count);
    for _ in 0..count {
        ords.push(dv.next_ord().map_err(io::Error::other)?);
    }
    // ALLOC: one term a value, at most a keyword's length.
    ords.into_iter()
        .map(|ord| {
            dv.lookup_ord(ord)
                .map(Value::Term)
                .map_err(io::Error::other)
        })
        // ALLOC: one term a value, as above.
        .collect()
}

#[cfg(test)]
#[path = "docvalue_fields_tests.rs"]
mod tests;
