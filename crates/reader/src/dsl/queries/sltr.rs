//! The `sltr` query of the learning to rank plugin: a stored model's score
//! of a hit, as the plugin's `StoredLtrQueryBuilder` reads it. Served as the
//! `rescore_query` of a `rescore` ([`crate::search_ltr`]); anywhere else it
//! is refused. With a `featureset` instead of a `model` it is the marker
//! feature logging reads (`ltr_log`), and `_name` is what a log names it by.

use crate::dsl::error::DslError;
use crate::dsl::queries::More;
use crate::dsl::tokens::{Kind, Parser};
use crate::template::{Val, read_val};

/// A parsed `sltr` query.
#[derive(Clone, Debug, PartialEq)]
pub struct Sltr {
    /// The store, the default one when absent.
    pub store: Option<String>,
    /// The model's name; empty for a logging query.
    pub model: String,
    /// The feature set a logging query reads, instead of a model.
    pub featureset: Option<String>,
    /// `_name`, the name a log asks for it by.
    pub name: Option<String>,
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
    let (mut featureset, mut qname) = (None, None);
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
                    "featureset" => featureset = Some(string(p, "featureset")?),
                    "_name" => qname = Some(string(p, "_name")?),
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
    if model.is_some() == featureset.is_some() {
        return Err(bad(p, "needs one of a [model] or a [featureset]"));
    }
    let model = model.unwrap_or_default();
    // ALLOC: one box an sltr query, bounded by the body's recording.
    Ok(More::Sltr(Box::new(Sltr {
        store,
        model,
        featureset,
        name: qname,
        params,
        active_features: active,
    })))
}
