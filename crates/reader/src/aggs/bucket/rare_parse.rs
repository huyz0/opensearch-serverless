//! `rare_terms`' body: `RareTermsAggregationBuilder`'s `ObjectParser` and its
//! setters' checks at OpenSearch 3.10.0.

use crate::aggs::bucket::rare::RareSpec;
use crate::aggs::metric::parse::long_value;
use crate::dsl::error::DslError;
use crate::dsl::tokens::{Kind, Parser};
use crate::dsl::values::suggest;

/// Every key `rare_terms` declares.
const KEYS: &[&str] = &[
    "field",
    "missing",
    "value_type",
    "format",
    "script",
    "max_doc_count",
    "precision",
    "include",
    "exclude",
];

/// The most `max_doc_count` may be (`MAX_MAX_DOC_COUNT`).
const MAX_MAX_DOC_COUNT: i64 = 100;

/// The field and options of a `rare_terms` body, the parser on its start.
pub fn parse_rare_body(p: &mut Parser<'_>, name: &str) -> Result<(String, RareSpec), DslError> {
    let mut field: Option<String> = None;
    let mut spec = RareSpec::default();
    let mut key = "";
    let mut key_at = p.location();
    loop {
        let kind = match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                key = p.text();
                key_at = p.location();
                continue;
            }
            Some(kind) => kind,
        };
        if !KEYS.contains(&key) {
            return Err(DslError::xcontent(
                key_at,
                // ALLOC: error path; one message.
                &format!("[rare_terms] unknown field [{key}]{}", suggest(key, KEYS)),
                None,
            ));
        }
        if !matches!(key, "field" | "max_doc_count" | "precision") {
            // ALLOC: error path; one message.
            return Err(DslError::unsupported(&format!(
                "the [rare_terms] aggregation's [{key}]"
            )));
        }
        let accepted = match key {
            "field" => kind == Kind::String,
            _ => matches!(kind, Kind::Number | Kind::String),
        };
        if !accepted {
            return Err(DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!(
                    "[rare_terms] {key} doesn't support values of type: {}",
                    kind.name()
                ),
                None,
            ));
        }
        let failed = |p: &Parser<'_>, cause: DslError| {
            DslError::xcontent(
                p.location(),
                // ALLOC: error path; one message.
                &format!("[rare_terms] failed to parse field [{key}]"),
                Some(cause),
            )
        };
        match key {
            // ALLOC: one field name an aggregation, bounded by the body's recording.
            "field" => field = Some(p.text().to_owned()),
            "max_doc_count" => {
                let n = long_value(kind, p.text()).map_err(|e| failed(p, e))?;
                if n < 1 {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!(
                                "[max_doc_count] must be greater than 0. Found [{n}] in [{name}]"
                            ),
                        ),
                    ));
                }
                if n > MAX_MAX_DOC_COUNT {
                    return Err(failed(
                        p,
                        DslError::illegal(
                            // ALLOC: error path; one message.
                            format!("[max_doc_count] must be smallerthan 100in [{name}]"),
                        ),
                    ));
                }
                spec.max_doc_count = u64::try_from(n).unwrap_or(1);
            }
            _ => {
                let text = p.text();
                let precision: f64 = text.parse().map_err(|_| {
                    failed(
                        p,
                        // ALLOC: error path; one message.
                        DslError::illegal(format!("For input string: \"{text}\"")),
                    )
                })?;
                if precision.is_nan() || precision < 0.00001 {
                    return Err(failed(
                        p,
                        DslError::illegal("[precision] must be greater than 0.00001"),
                    ));
                }
                spec.precision = precision;
            }
        }
    }
    let field = field.ok_or_else(|| {
        DslError::illegal("Required one of fields [field, script], but none were specified. ")
    })?;
    Ok((field, spec))
}
