//! What the query-string leaves share: `wrapWithBoost`, and the test for a
//! text that analyzes to nothing, which `ZeroTermsQuery.NULL` turns into a
//! dropped clause.

use lucene_search::query_builder::QueryBuilder;
use lucene_search::query_visitor::Occur;
use lucene_search::{BoostQuery, Clause};

use crate::lucene_query::QueryContext;
use crate::mapping::FieldType;

/// Whether `text` has no term to search in any of `fields`: only text
/// fields drop it, any other type always has its term. `slop` is the
/// phrase's, or `None` for words.
pub(super) fn no_terms<'f>(
    cx: &QueryContext<'_>,
    mut fields: impl Iterator<Item = &'f str>,
    text: &str,
    slop: Option<u32>,
) -> bool {
    let builder = QueryBuilder::new(cx.analyzer);
    fields.all(|field| {
        let text_field = cx
            .mapping
            .get(field)
            .is_some_and(|f| f.ty == FieldType::Text);
        text_field
            && match slop {
                None => matches!(
                    builder.create_boolean_query_with(field, text, Occur::Should),
                    Ok(None)
                ),
                Some(s) => matches!(
                    builder.create_phrase_query_with_slop(field, text, s),
                    Ok(None)
                ),
            }
    })
}

/// `wrapWithBoost`: a query a boost applies to, unless it matches nothing.
pub(super) fn boosted(query: Clause, boost: f32) -> Clause {
    if matches!(query, Clause::MatchNoDocs(_)) || boost == 1.0 {
        query
    } else {
        // ALLOC: one box a boosted field's query.
        Clause::Boost(Box::new(BoostQuery::new(query, boost)))
    }
}

/// Whether `field` is a text or keyword field of the mapping.
pub(super) fn cx_is_string(cx: &QueryContext<'_>, field: &str) -> bool {
    cx.mapping
        .get(field)
        .is_some_and(|f| matches!(f.ty, FieldType::Text | FieldType::Keyword))
}

/// `Float.parseFloat` of the digits a fuzzy token ends in, with the
/// suffixes Java accepts.
pub(super) fn java_float(s: &str) -> Option<f32> {
    let s = s.strip_suffix(['f', 'F', 'd', 'D']).unwrap_or(s);
    s.parse::<f32>().ok().filter(|f| f.is_finite())
}

/// `Fuzziness.fromEdits((int) similarity)`'s distance: at most 2.
pub(super) fn edits_of(similarity: f32) -> u8 {
    // The cast saturates as Java's does; the clamp keeps 0..=2.
    #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
    let n = similarity as i32;
    u8::try_from(n.clamp(0, 2)).unwrap_or(2)
}
