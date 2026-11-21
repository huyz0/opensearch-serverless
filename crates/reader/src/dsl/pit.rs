//! The `pit` of a search body: the point in time the search runs on.
//!
//! Ported from `PointInTimeBuilder`'s parser: `id` is required, and
//! `keep_alive` is a time value kept as written.

use super::error::DslError;
use super::tokens::{Kind, Parser};
use super::values::text_or_null;

/// A point in time a search names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PitRef {
    /// The view's id.
    pub id: String,
    /// `keep_alive`, as written.
    pub keep_alive: Option<String>,
}

fn failure(p: &Parser<'_>, reason: &str) -> DslError {
    DslError::parsing(p.location(), reason)
}

/// Reads the object `p` is on the start of.
pub(crate) fn parse_pit(p: &mut Parser<'_>) -> Result<PitRef, DslError> {
    let mut id: Option<String> = None;
    let mut keep_alive: Option<String> = None;
    let mut name = "";
    while let Some(token) = p.next_token() {
        match token {
            Kind::EndObject => break,
            Kind::FieldName => name = p.text(),
            Kind::String if name == "id" => {
                // ALLOC: one id a search that names a point in time.
                id = Some(p.text().to_owned());
            }
            Kind::String | Kind::Null if name == "keep_alive" => {
                // ALLOC: one keep-alive a search that names one.
                keep_alive = text_or_null(p).map(str::to_owned);
            }
            Kind::StartObject | Kind::StartArray => {
                return Err(failure(
                    p,
                    // ALLOC: error path; one message.
                    &format!("[pit] [{name}] doesn't support values of this type"),
                ));
            }
            _ if name == "id" || name == "keep_alive" => {
                return Err(failure(
                    p,
                    // ALLOC: error path; one message.
                    &format!(
                        "[pit] {name} doesn't support values of type: {}",
                        token.name()
                    ),
                ));
            }
            _ => {
                // ALLOC: error path; one message.
                return Err(failure(p, &format!("[pit] unknown field [{name}]")));
            }
        }
    }
    let id = id.ok_or_else(|| failure(p, "[pit] Required [id]"))?;
    Ok(PitRef { id, keep_alive })
}
