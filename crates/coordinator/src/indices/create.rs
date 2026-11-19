//! What a create-index request asks for: its shard count, mapping, settings
//! and aliases, and which asks are refused before any index exists.

use oss_doc::mapping::json::{Json, parse as parse_json};

use super::flat::flatten;
use crate::route::resolve::Refusal;

/// The most shards an index may have.
pub const MAX_SHARDS: u32 = 4096;

const SHARDS_KEY: &str = "index.number_of_shards";
const REPLICAS_KEY: &str = "index.number_of_replicas";

/// A create-index request, read.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateRequest {
    /// The shard count to create with.
    pub shards: u32,
    /// Whether the caller named a shard count, as opposed to the default of one.
    pub explicit_shards: bool,
    /// The mapping as JSON text, when the request had one.
    pub mapping: Option<String>,
    /// The settings carried into the descriptor, shard and replica counts removed.
    pub settings: Vec<(String, Option<String>)>,
    /// The aliases the body names, each with its options in order.
    pub aliases: Vec<(String, Vec<(String, Json)>)>,
}

fn malformed(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "malformed_body", reason)
}

/// Reads a create-index request: the query's `shards` (1 when it gave none)
/// and the body, if there is one.
///
/// # Errors
///
/// A [`Refusal`] for a body that is not a JSON object, an envelope part that
/// is not an object, a replica count other than zero, a shard count that is
/// not a number or outside `1..=MAX_SHARDS`, and an alias option this design
/// cannot honour.
pub fn parse(query_shards: i32, body: Option<&[u8]>) -> Result<CreateRequest, Refusal> {
    let body = body.filter(|b| !b.is_empty());
    let Some(body) = body else {
        return settled(query_shards, None, Vec::default(), Vec::default());
    };
    let members = match parse_json(body) {
        Ok(Json::Obj(members)) => members,
        _ => return Err(malformed("the body is not a JSON object")),
    };
    let named = |name: &str| members.iter().any(|(key, _)| key == name);
    if !(named("settings") || named("mappings") || named("aliases")) {
        // The shape the handler has always taken: the body is the mapping.
        let text = String::from_utf8_lossy(body).into_owned();
        return settled(query_shards, Some(text), Vec::default(), Vec::default());
    }
    let aliases = aliases(&members)?;
    let mapping = part(&members, "mappings")?
        .and(members.iter().find(|(key, _)| key == "mappings"))
        .map(|(_, value)| value.to_json());
    let settings = match part(&members, "settings")? {
        Some(raw) => flatten(raw),
        None => Vec::default(),
    };
    settled(query_shards, mapping, settings, aliases)
}

/// The object named `name`, nothing when absent, refused when it is not an object.
fn part<'a>(
    members: &'a [(String, Json)],
    name: &str,
) -> Result<Option<&'a [(String, Json)]>, Refusal> {
    match members.iter().find(|(key, _)| key == name) {
        None => Ok(None),
        Some((_, Json::Obj(inner))) => Ok(Some(inner)),
        // ALLOC: one message, bounded by the part's name.
        Some(_) => Err(malformed(format!("'{name}' must be an object"))),
    }
}

type Aliases = Vec<(String, Vec<(String, Json)>)>;

fn aliases(members: &[(String, Json)]) -> Result<Aliases, Refusal> {
    let Some(named) = part(members, "aliases")? else {
        return Ok(Vec::default());
    };
    // ALLOC: one entry per alias in the body, which the edge bounds.
    let mut out = Vec::with_capacity(named.len());
    for (name, given) in named {
        let options = match given {
            Json::Obj(options) => options.clone(),
            _ => Vec::default(),
        };
        if let Some(reason) = unsupported_alias_option(&options) {
            return Err(Refusal::new(501, "unsupported_alias_option", reason));
        }
        out.push((name.clone(), options));
    }
    Ok(out)
}

/// The reason an alias option cannot be honoured, when one cannot.
pub fn unsupported_alias_option(options: &[(String, Json)]) -> Option<&'static str> {
    let set = |name: &str| {
        options
            .iter()
            .any(|(key, value)| key == name && *value != Json::Null)
    };
    if set("filter") {
        return Some(
            "filter is not supported: an alias here names indices and does not narrow them",
        );
    }
    if set("routing") || set("index_routing") || set("search_routing") {
        return Some("routing on an alias is not supported: a document is placed by its id alone");
    }
    let hidden = options
        .iter()
        .any(|(key, value)| key == "is_hidden" && *value == Json::Bool(true));
    if hidden {
        return Some(
            "is_hidden is not supported: aliases are found by name and never listed, so there is nothing to hide from",
        );
    }
    None
}

fn value_of<'a>(settings: &'a [(String, Option<String>)], key: &str) -> Option<&'a str> {
    settings
        .iter()
        .find(|(name, _)| name == key)
        .and_then(|(_, value)| value.as_deref())
}

/// The shard and replica checks, and the settings that are left.
fn settled(
    query_shards: i32,
    mapping: Option<String>,
    settings: Vec<(String, Option<String>)>,
    aliases: Aliases,
) -> Result<CreateRequest, Refusal> {
    if value_of(&settings, REPLICAS_KEY).is_some_and(|replicas| replicas != "0") {
        return Err(Refusal::new(
            501,
            "unsupported_setting",
            "number_of_replicas is not supported: a shard has one writer and its durability comes from \
the object store, not from replica copies. Readers are added by scaling search nodes, \
not by setting a replica count.",
        ));
    }
    let requested = value_of(&settings, SHARDS_KEY);
    let named_shards = requested.is_some();
    let shards = match requested {
        None => query_shards,
        Some(text) => text
            .parse::<i32>()
            .map_err(|_| malformed("number_of_shards must be a number"))?,
    };
    let shards = u32::try_from(shards)
        .ok()
        .filter(|n| (1..=MAX_SHARDS).contains(n))
        .ok_or_else(|| {
            Refusal::new(
                400,
                "invalid_shard_count",
                // ALLOC: one message, bounded by the template's length.
                format!("number_of_shards must be between 1 and {MAX_SHARDS}, got {shards}"),
            )
        })?;
    let settings = settings
        .into_iter()
        .filter(|(key, _)| key != SHARDS_KEY && key != REPLICAS_KEY)
        // ALLOC: the settings again, less the two consumed.
        .collect();
    Ok(CreateRequest {
        shards,
        explicit_shards: named_shards || query_shards != 1,
        mapping,
        settings,
        aliases,
    })
}
