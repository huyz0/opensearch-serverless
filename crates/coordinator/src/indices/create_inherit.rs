//! What a create request carries once the template it inherits from is
//! layered under it.
//!
//! Ported from `serverless/shell/src/main/java/org/opensearch/serverless/indexadmin/IndexCreateRequest.java`
//! (`shardsWith`, `settingsWith`, `mappingWith`). The template's settings sit
//! under the request's, its mappings are merged under the request's with the
//! template-merge rule (a field definition replaces rather than merging
//! parameter by parameter), and its shard count stands unless the request
//! named one. Replicas in a template are dropped, as the Java drops them:
//! a request that asks for them is refused, a template that carries them is
//! not.

use oss_doc::mapping::json::{Json, parse as parse_json};

use super::create::{CreateRequest, MAX_SHARDS};
use super::flat::flatten;
use crate::route::resolve::Refusal;
use crate::templates::{Inherited, merge_mappings};

const SHARDS_KEY: &str = "index.number_of_shards";
const REPLICAS_KEY: &str = "index.number_of_replicas";
const TYPE_NAME: &str = "_doc";

fn malformed(reason: &'static str) -> Refusal {
    Refusal::new(400, "malformed_body", reason)
}

fn members(inherited: &Json) -> &[(String, Json)] {
    match inherited {
        Json::Obj(members) => members,
        _ => &[],
    }
}

/// Layers what `inherited` carries under `request`.
///
/// A request no template matched is returned as it is.
///
/// # Errors
///
/// A [`Refusal`] for a template shard count outside `1..=MAX_SHARDS`, and
/// for a request mapping that is not one object (or one `_doc` object).
pub fn inherit(
    mut request: CreateRequest,
    inherited: &Inherited,
) -> Result<CreateRequest, Refusal> {
    if inherited.from.is_none() {
        return Ok(request);
    }
    let from_template = flatten(members(&inherited.settings));
    if !request.explicit_shards
        && let Some(count) = from_template
            .iter()
            .find(|(key, _)| key == SHARDS_KEY)
            .and_then(|(_, value)| value.as_deref())
            .and_then(|text| text.parse::<i32>().ok())
    {
        request.shards = u32::try_from(count)
            .ok()
            .filter(|n| (1..=MAX_SHARDS).contains(n))
            .ok_or_else(|| {
                Refusal::new(
                    400,
                    "invalid_shard_count",
                    // ALLOC: one message, once, on a refusal.
                    format!("number_of_shards must be between 1 and {MAX_SHARDS}, got {count}"),
                )
            })?;
    }
    // ALLOC: the template's settings less two, at most the template's size.
    let mut settings: Vec<(String, Option<String>)> = from_template
        .into_iter()
        .filter(|(key, _)| key != SHARDS_KEY && key != REPLICAS_KEY)
        .collect(); // ALLOC: as above
    for (key, value) in std::mem::take(&mut request.settings) {
        match settings.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => settings.push((key, value)),
        }
    }
    settings.sort_by(|a, b| a.0.cmp(&b.0));
    request.settings = settings;
    request.mapping = mapping_with(request.mapping.take(), inherited)?;
    Ok(request)
}

/// The request's mapping as an object, its `_doc` wrapper removed.
fn normalized(text: &str) -> Result<Vec<(String, Json)>, Refusal> {
    let Ok(Json::Obj(mut mapping)) = parse_json(text.as_bytes()) else {
        return Err(malformed("mappings must be an object"));
    };
    if mapping.iter().any(|(key, _)| key == TYPE_NAME) {
        return match (mapping.len(), mapping.pop()) {
            (1, Some((_, Json::Obj(inner)))) => Ok(inner),
            _ => Err(malformed("a typed mapping must contain one mapping object")),
        };
    }
    Ok(mapping)
}

fn mapping_with(
    requested: Option<String>,
    inherited: &Inherited,
) -> Result<Option<String>, Refusal> {
    let own = match requested.as_deref().filter(|text| !text.is_empty()) {
        Some(text) => Some(normalized(text)?),
        None => None,
    };
    let from_template = members(&inherited.mappings);
    if from_template.is_empty() {
        return Ok(own.map(|m| Json::Obj(m).to_json()));
    }
    // ALLOC: a copy of the template's mappings, bounded by the template.
    let mut merged = from_template.to_vec();
    if let Some(own) = own {
        merge_mappings(&mut merged, &own);
    }
    Ok(Some(Json::Obj(merged).to_json()))
}
