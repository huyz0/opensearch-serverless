//! The `sltr` query of the learning to rank plugin: a stored model's score
//! of a hit, as the plugin's `StoredLtrQueryBuilder` reads it. Served as the
//! `rescore_query` of a `rescore` ([`crate::search_ltr`]); anywhere else it
//! is refused. The parameters that name a feature set or a log (`featureset`,
//! `_name`) belong to feature logging and are refused until that is served.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::tokens::{Kind, Parser};
use crate::template::{Val, read_val};

/// A parsed `sltr` query.
#[derive(Clone, Debug, PartialEq)]
pub struct Sltr {
    /// The store, the default one when absent.
    pub store: Option<String>,
    /// The model's name.
    pub model: String,
    /// The parameters the feature templates render with.
    pub params: Val,
    /// The features to compute, all when absent.
    pub active_features: Option<Vec<String>>,
}

fn bad(p: &Parser<'_>, reason: &str) -> DslError {
    // ALLOC: one string, bounded by the body's recording.
    DslError::parsing(p.location(), format!("[sltr] {reason}"))
}

fn string(p: &Parser<'_>, name: &str) -> Result<String, DslError> {
    if p.current_token() != Some(Kind::String) {
        // ALLOC: one string, bounded by the body's recording.
        return Err(bad(p, &format!("[{name}] must be a string")));
    }
    // ALLOC: one string, bounded by the body's recording.
    Ok(p.text().to_owned())
}

/// `{"sltr": {...}}`: the parser is on the object's start.
pub fn parse_sltr(p: &mut Parser<'_>) -> Result<More, DslError> {
    // ALLOC: one string, bounded by the body's recording.
    let (mut store, mut model, mut params, mut active) = (None, None, Val::Map(Vec::new()), None);
    loop {
        match p.next_token() {
            Some(Kind::EndObject) | None => break,
            Some(Kind::FieldName) => {
                // ALLOC: one string, bounded by the body's recording.
                let name = p.text().to_owned();
                p.next_token();
                match name.as_str() {
                    "model" => model = Some(string(p, "model")?),
                    "store" => store = Some(string(p, "store")?),
                    "params" => {
                        if p.current_token() != Some(Kind::StartObject) {
                            return Err(bad(p, "[params] must be an object"));
                        }
                        params = read_val(p, 0)?;
                    }
                    "active_features" => {
                        if p.current_token() != Some(Kind::StartArray) {
                            return Err(bad(p, "[active_features] must be an array"));
                        }
                        // ALLOC: one name a feature, bounded by the body's recording.
                        let mut names = Vec::new();
                        while let Some(kind) = p.next_token() {
                            match kind {
                                Kind::EndArray => break,
                                // ALLOC: one string, bounded by the body's recording.
                                Kind::String => names.push(p.text().to_owned()),
                                _ => return Err(bad(p, "[active_features] holds strings")),
                            }
                        }
                        active = Some(names);
                    }
                    "featureset" | "_name" => {
                        // ALLOC: one string, bounded by the body's recording.
                        return Err(DslError::unsupported(&format!(
                            "[sltr] with [{name}], feature logging,"
                        )));
                    }
                    other => {
                        return Err(DslError::parsing(
                            p.location(),
                            // ALLOC: one string, bounded by the body's recording.
                            format!("[sltr] unknown field [{other}]"),
                        ));
                    }
                }
            }
            Some(_) => return Err(bad(p, "malformed")),
        }
    }
    let Some(model) = model else {
        return Err(bad(p, "needs a [model]"));
    };
    // ALLOC: one box an sltr query, bounded by the body's recording.
    Ok(More::Sltr(Box::new(Sltr {
        store,
        model,
        params,
        active_features: active,
    })))
}
