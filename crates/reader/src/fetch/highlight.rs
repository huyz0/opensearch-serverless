//! `highlight`: each hit's fields with their matches marked, as
//! `HighlightPhase` and the `UnifiedHighlighter` of OpenSearch 3.10.0 give
//! them, over the engine's port of the Lucene highlighters.
//!
//! The text of a field is re-analysed (the offset source of a field mapped
//! without offsets or term vectors, which is every field this reader reads):
//! the values at the field's path in the stored source are joined with the
//! NUL character OpenSearch separates them by, and the engine cuts the
//! passages. A field with nothing to show is left out; the fields that
//! show something come out as a `java.util.HashMap` writes them.

use std::io::{self, Write};
use std::sync::Arc;

use lucene_analysis::Analyzer;
use lucene_highlighter::highlighter::OffsetSource;
use lucene_highlighter::unified::opensearch::{
    ALL_PASSAGES, Breaks, CustomUnifiedHighlighter, Options as EngineOptions,
};
use lucene_highlighter::unified::strategies::MULTIVAL_SEP_CHAR;
use lucene_search::{BooleanQuery, Clause};
use oss_rest::write_json_str;

use crate::aggs::metric::double::write_java_double;
use crate::dsl::queries::build::simple_match;
use crate::dsl::source::tokenizer;
use crate::dsl::tokens::{Kind, Recorder};
use crate::failure::Failure;
use crate::fetch::highlight_parse::{HighlightSpec, Options};
use crate::fetch::source_filter::scratch_bytes;
use crate::fetch::source_map::{Value, java_hash_order, read};
use crate::mapping::{FieldType, Mapping};
use crate::search::ShardError;

/// `index.highlight.max_analyzed_offset`'s default: the most characters of
/// a field that are re-analysed.
pub const MAX_ANALYZED_OFFSET: usize = 1_000_000;

/// `CustomUnifiedHighlighter.MULTIVAL_SEP_CHAR`: the separator between a
/// field's values.
const SEPARATOR: u16 = MULTIVAL_SEP_CHAR;

/// One field to highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldPlan {
    /// The field's full name.
    pub name: String,
    /// Whether it is a keyword field (one token, no sentences).
    pub keyword: bool,
    /// Its options.
    pub options: Options,
}

/// What a search highlights: the query and the fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The query the matches are taken from.
    pub query: Clause,
    /// The fields, in the order their fetch builders are made.
    pub fields: Vec<FieldPlan>,
}

fn refused(reason: String) -> ShardError {
    ShardError {
        status: 400,
        failure: Failure::plain("illegal_argument_exception", reason),
    }
}

/// `HighlightPhase.contextBuilders`: the mapped fields each entry names. A
/// pattern keeps only text and keyword fields; a name that is not mapped is
/// skipped, and a name of another type is not served yet.
///
/// # Errors
/// A named field of a type that is neither text nor keyword.
pub fn plan(
    spec: &HighlightSpec,
    mapping: &Mapping,
    query: &BooleanQuery,
) -> Result<Option<Plan>, ShardError> {
    // ALLOC: one entry a distinct field, bounded by the mapping's fields.
    let mut fields: Vec<FieldPlan> = Vec::new();
    for (entry, options) in &spec.fields {
        // ALLOC: one name a mapped field, at most the mapping's fields.
        let names: Vec<&str> = if entry.contains('*') {
            mapping.names().filter(|n| simple_match(entry, n)).collect()
        } else {
            // ALLOC: as above.
            mapping
                .get(entry)
                .map(|_| entry.as_str())
                .into_iter()
                // ALLOC: one name a mapped field, at most the mapping's fields.
                .collect()
        };
        for name in names {
            let keyword = match mapping.get(name).map(|f| &f.ty) {
                Some(FieldType::Text) => false,
                Some(FieldType::Keyword) => true,
                _ if entry.contains('*') => continue,
                _ => {
                    // ALLOC: error path; one message.
                    return Err(refused(format!(
                        "highlighting [{name}], which is not a text or keyword field, is not \
                         supported by this node yet"
                    )));
                }
            };
            let planned = FieldPlan {
                // ALLOC: one name a distinct field, as above.
                name: name.to_owned(),
                keyword,
                // ALLOC: one set of tags a distinct field, as above.
                options: options.clone(),
            };
            match fields.iter_mut().find(|f| f.name == name) {
                Some(f) => *f = planned,
                None => fields.push(planned),
            }
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    Ok(Some(Plan {
        // ALLOC: one copy of the query, bounded by its recording.
        query: Clause::Boolean(Box::new(query.clone())),
        fields,
    }))
}

/// The engine's highlighter for each planned field.
pub struct Highlighters<'p> {
    fields: Vec<(&'p FieldPlan, CustomUnifiedHighlighter)>,
}

impl<'p> Highlighters<'p> {
    /// `UnifiedHighlighter.buildHighlighter` for each field of `plan`.
    ///
    /// # Errors
    /// The engine's message.
    pub fn new(plan: &'p Plan) -> io::Result<Self> {
        // ALLOC: one highlighter a field, bounded by the mapping's fields.
        let mut fields = Vec::with_capacity(plan.fields.len());
        for f in &plan.fields {
            let o = &f.options;
            let (breaks, max_passages) = if o.number_of_fragments == 0 || f.keyword {
                (
                    Breaks::Values,
                    if o.number_of_fragments == 0 {
                        ALL_PASSAGES
                    } else {
                        o.number_of_fragments
                    },
                )
            } else if o.word_boundary {
                (Breaks::Word, o.number_of_fragments)
            } else if o.fragment_size > 0 {
                (
                    Breaks::BoundedSentence(o.fragment_size),
                    o.number_of_fragments,
                )
            } else {
                (Breaks::Sentence, o.number_of_fragments)
            };
            let name = f.name.clone();
            let matcher: lucene_highlighter::unified::strategies::FieldMatcher =
                if o.require_field_match {
                    Arc::new(move |field: &str| field == name)
                } else {
                    Arc::new(|field: &str| field != "_id")
                };
            let engine = EngineOptions {
                // ALLOC: one name a field, as above.
                field: f.name.clone(),
                offset_source: OffsetSource::Analysis,
                breaks,
                max_passages,
                no_match_size: o.no_match_size,
                // ALLOC: one tag a field, as above.
                pre_tag: o.pre_tag.clone(),
                // ALLOC: as above.
                post_tag: o.post_tag.clone(),
                html: o.html,
                field_matcher: matcher,
                masked_fields: None,
            };
            let analyzer = if f.keyword {
                Analyzer::keyword()
            } else {
                Analyzer::standard(None)
            };
            let h = CustomUnifiedHighlighter::new(engine, &plan.query, Some(Arc::new(analyzer)))
                .map_err(io::Error::other)?;
            fields.push((f, h));
        }
        Ok(Self { fields })
    }

    /// Writes `,"highlight":{...}` for the hit whose stored source is
    /// `source`, or nothing when no field has anything to show.
    ///
    /// # Errors
    /// A source that is not a JSON object or is too long to re-analyse, a
    /// failure of the engine, or a write failure.
    pub fn write(&mut self, out: &mut dyn Write, source: &[u8]) -> io::Result<()> {
        if source.len() > MAX_ANALYZED_OFFSET {
            return Err(io::Error::other(
                "highlighting a _source over 1,000,000 bytes is not supported by this node yet",
            ));
        }
        // ALLOC: the source's tokens and text, bounded by the stored source,
        // which the caller reserved.
        let mut rec = Recorder::new(scratch_bytes(source.len()));
        let mut tokens = tokenizer();
        tokens
            .feed(source, &mut rec)
            .map_err(|e| io::Error::other(e.into_failure().reason))?;
        tokens
            .finish(&mut rec)
            .map_err(|e| io::Error::other(e.into_failure().reason))?;
        let mut p = rec.parser();
        let Some(Kind::StartObject) = p.next_token() else {
            return Err(io::Error::other("a _source that is not a JSON object"));
        };
        let tree = read(&mut p, Kind::StartObject);
        // ALLOC: one entry a field that shows something.
        let mut shown: Vec<(&str, Vec<String>)> = Vec::new();
        for (plan, highlighter) in &mut self.fields {
            // ALLOC: one string a value of the field, bounded by the source.
            let mut values = Vec::new();
            let path: Vec<&str> = plan.name.split('.').collect();
            collect(&tree, &path, &mut values);
            if values.is_empty() {
                continue;
            }
            // `mergeFieldValues`: the values joined by the separator.
            // ALLOC: the values of one field, bounded by the stored source.
            let mut content: Vec<u16> = Vec::new();
            for v in &values {
                content.extend(v.encode_utf16());
                content.push(SEPARATOR);
            }
            content.pop();
            if highlighter.is_noop() && highlighter.no_match_size() == 0 {
                continue;
            }
            let snippets = highlighter
                .highlight(None, &content)
                .map_err(io::Error::other)?;
            // ALLOC: one fragment a passage, at most the field's fragments.
            let mut kept: Vec<_> = snippets
                .into_iter()
                .map(|s| (s.score, String::from_utf16_lossy(&s.text)))
                .filter(|(_, text)| text.chars().any(|c| !c.is_whitespace()))
                // ALLOC: one fragment a passage, at most the field's fragments.
                .collect();
            if kept.is_empty() {
                continue;
            }
            if plan.options.score_ordered {
                kept.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            // ALLOC: one fragment a passage, as above.
            shown.push((&plan.name, kept.into_iter().map(|(_, t)| t).collect()));
        }
        if shown.is_empty() {
            return Ok(());
        }
        // ALLOC: one name a field that shows something.
        let names: Vec<&str> = shown.iter().map(|(n, _)| *n).collect();
        // ALLOC: one index a field that shows something.
        let mut order = Vec::new();
        java_hash_order(&names, &mut order);
        out.write_all(b",\"highlight\":{")?;
        for (n, &i) in order.iter().enumerate() {
            let Some((name, fragments)) = shown.get(i) else {
                continue;
            };
            if n > 0 {
                out.write_all(b",")?;
            }
            write_json_str(out, name)?;
            out.write_all(b":[")?;
            for (k, f) in fragments.iter().enumerate() {
                if k > 0 {
                    out.write_all(b",")?;
                }
                write_json_str(out, f)?;
            }
            out.write_all(b"]")?;
        }
        out.write_all(b"}")
    }
}

/// The values a path reads from a source, each as Java prints it: a string
/// as it is, a number and a boolean as their text, an array flattened.
fn collect(v: &Value<'_>, path: &[&str], out: &mut Vec<String>) {
    match (v, path.split_first()) {
        (Value::Array(items), _) => {
            for item in items {
                collect(item, path, out);
            }
        }
        (Value::Object(entries), Some((first, rest))) => {
            if let Some((_, child)) = entries.iter().find(|(k, _)| k == first) {
                collect(child, rest, out);
            }
        }
        (Value::Str(s), None) => {
            // ALLOC: one string a value, bounded by the source.
            out.push((*s).to_owned());
        }
        (Value::Num(text), None) => {
            // ALLOC: one number's text, bounded by the source.
            let mut b = Vec::new();
            let written = if text.bytes().any(|c| matches!(c, b'.' | b'e' | b'E')) {
                write_java_double(&mut b, text.parse::<f64>().unwrap_or(f64::NAN))
            } else {
                b.write_all(text.as_bytes())
            };
            if written.is_ok() {
                // ALLOC: one string a value, bounded by the source.
                out.push(String::from_utf8_lossy(&b).into_owned());
            }
        }
        (Value::Bool(b), None) => {
            // ALLOC: one string a value, bounded by the source.
            out.push(b.to_string());
        }
        _ => {}
    }
}

/// The bytes of scratch highlighting a source of `len` bytes holds at once,
/// beyond reading it: the value tree, the text as UTF-16, and the marked-up
/// passages, each at most the text with a pair of tags for every character.
/// A bound, not an estimate.
pub fn scratch_for(len: usize, plan: &Plan) -> usize {
    let tags = plan
        .fields
        .iter()
        .map(|f| {
            f.options
                .pre_tag
                .len()
                .saturating_add(f.options.post_tag.len())
        })
        .max()
        .unwrap_or(0);
    scratch_bytes(len).saturating_add(len.saturating_mul(tags.saturating_add(1).saturating_mul(4)))
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
