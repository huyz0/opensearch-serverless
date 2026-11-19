//! A rollover decided from its body and what the target says: the conditions
//! and the shape asked of the new index, the next index's name, the refusals
//! on what the target is, and whether the conditions say to roll.

use oss_doc::mapping::json::{Json, parse as parse_json};

use super::create::{CreateRequest, parse as parse_create};
use super::flat::flatten;
use super::time_value::{TimeValue, parse as parse_time};
use crate::route::resolve::Refusal;

/// The members a rollover body may carry, each an object.
const BODY_FIELDS: [&str; 4] = ["conditions", "settings", "mappings", "aliases"];

/// The prefix of a data stream's backing index.
const BACKING_INDEX_PREFIX: &str = ".ds-";

/// One condition a rollover names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    /// `max_age`: the current index is at least this old.
    MaxAge(TimeValue),
    /// `max_docs`: the current index holds at least this many documents.
    MaxDocs(i64),
}

/// A rollover body, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    /// The conditions, in the order the body wrote them.
    pub conditions: Vec<Condition>,
    /// What the body asks of the new index, when it names settings, mappings
    /// or aliases.
    pub create: Option<CreateRequest>,
    /// Whether the body names a mapping or any setting, which a data stream
    /// refuses.
    pub shapes_index: bool,
}

impl Ask {
    /// The refusal for aliases in the body: the new index would be created
    /// without the aliases asked for, so a rollover that would roll refuses.
    pub fn aliases_refusal(&self) -> Option<Refusal> {
        let named = self
            .create
            .as_ref()
            .is_some_and(|create| !create.aliases.is_empty());
        named.then(|| {
            Refusal::new(
                501,
                "unsupported_setting",
                "'aliases' in a rollover body is not supported here; add them to the new index with POST /_aliases",
            )
        })
    }
}

fn malformed(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "malformed_body", reason)
}

/// Reads a rollover body.
///
/// # Errors
///
/// A [`Refusal`] for a body that is not an object, a member that is not a
/// supported object, a condition that is unknown or cannot be evaluated here
/// or whose value does not read, and what a create request refuses.
pub fn parse_body(body: Option<&[u8]>) -> Result<Ask, Refusal> {
    let body = body.filter(|b| !b.is_empty());
    let Some(body) = body else {
        return Ok(Ask {
            conditions: Vec::default(),
            create: None,
            shapes_index: false,
        });
    };
    let members = match parse_json(body) {
        Ok(Json::Obj(members)) => members,
        _ => return Err(malformed("the body is not a JSON object")),
    };
    for (key, value) in &members {
        if !BODY_FIELDS.contains(&key.as_str()) || !matches!(value, Json::Obj(_)) {
            return Err(malformed(
                // ALLOC: one message, bounded by the member's name.
                format!("rollover field [{key}] must be a supported object-valued field"),
            ));
        }
    }
    let conditions = match members.iter().find(|(key, _)| key == "conditions") {
        Some((_, Json::Obj(named))) => conditions(named)?,
        _ => Vec::default(),
    };
    // ALLOC: the body's other members, bounded by the body.
    let rest: Vec<(String, Json)> = members
        .iter()
        .filter(|(key, _)| key != "conditions")
        .cloned()
        // ALLOC: one short string or list, bounded by the request body.
        .collect();
    if rest.is_empty() {
        return Ok(Ask {
            conditions,
            create: None,
            shapes_index: false,
        });
    }
    let text = Json::Obj(rest.clone()).to_json();
    let mut create = parse_create(1, Some(text.as_bytes()))?;
    // Core says `{}` for an absent mapping.
    if create.mapping.as_deref() == Some("{}") {
        create.mapping = None;
    }
    let sets = rest.iter().any(|(key, value)| match (key.as_str(), value) {
        ("settings", Json::Obj(settings)) => !flatten(settings).is_empty(),
        _ => false,
    });
    Ok(Ask {
        conditions,
        shapes_index: create.mapping.is_some() || sets,
        create: Some(create),
    })
}

fn conditions(named: &[(String, Json)]) -> Result<Vec<Condition>, Refusal> {
    for (name, _) in named {
        if name == "max_age" || name == "max_docs" {
            continue;
        }
        let size = name.starts_with("max_size") || name.starts_with("max_primary_shard_size");
        return Err(if size {
            Refusal::new(
                501,
                "unsupported_condition",
                "store sizes are not reported here, so a size condition cannot be evaluated",
            )
        } else {
            Refusal::new(
                400,
                "unsupported_condition",
                // ALLOC: one message, bounded by the condition's name.
                format!("unknown condition [{name}]"),
            )
        });
    }
    // ALLOC: one entry per condition, which the body bounds.
    let mut out = Vec::with_capacity(named.len());
    for (name, value) in named {
        let text = match value {
            Json::Str(s) | Json::Num(s) => s.clone(),
            other => other.to_json(),
        };
        if name == "max_age" {
            let age = parse_time(&text, "max_age").map_err(malformed)?;
            out.push(Condition::MaxAge(age));
        } else {
            let docs = text
                .parse::<i64>()
                .map_err(|_| malformed("max_docs must be a number"))?;
            out.push(Condition::MaxDocs(docs));
        }
    }
    Ok(out)
}

fn exhausted(what: &str, current: impl std::fmt::Display) -> Refusal {
    Refusal::new(
        400,
        "illegal_argument_exception",
        // ALLOC: one message.
        format!("{what} is exhausted at [{current}]"),
    )
}

/// Core's naming rule: the trailing number after the last dash, incremented
/// and padded to six digits. None when the name does not end that way.
///
/// # Errors
///
/// A [`Refusal`] when the counter cannot be incremented.
pub fn next_name(name: &str) -> Result<Option<String>, Refusal> {
    let Some(dash) = name.rfind('-') else {
        return Ok(None);
    };
    let (head, digits) = name.split_at(dash.saturating_add(1));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let next = digits
        .parse::<i64>()
        .ok()
        .and_then(|current| current.checked_add(1))
        .ok_or_else(|| exhausted("numeric index suffix", digits))?;
    // ALLOC: one index name.
    Ok(Some(format!("{head}{next:06}")))
}

/// A data stream's backing index at `generation`.
pub fn backing_index_name(stream: &str, generation: i64) -> String {
    // ALLOC: one index name.
    format!("{BACKING_INDEX_PREFIX}{stream}-{generation:06}")
}

/// What the target of a rollover is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An alias or a data stream.
    Alias(AliasFacts),
    /// A concrete index.
    Index,
    /// Nothing by that name.
    Missing,
}

/// What an alias record says that a rollover cares about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasFacts {
    /// Whether it is a data stream.
    pub data_stream: bool,
    /// Whether a mutation of it is pending.
    pub mutation_pending: bool,
    /// The index it writes to, when it has one.
    pub write_index: Option<String>,
}

/// Refuses a target that cannot be rolled over, in the shell's order.
///
/// # Errors
///
/// A [`Refusal`] for an index or a missing target, an alias with a mutation
/// pending, a data stream given a name or a shape, and an alias with no write
/// index.
pub fn check_target(
    name: &str,
    target: &Target,
    new_index: Option<&str>,
    shapes_index: bool,
) -> Result<(), Refusal> {
    let illegal = |reason: String| Refusal::new(400, "illegal_argument_exception", reason);
    let facts = match target {
        Target::Index => {
            // ALLOC: one short string or list, bounded by the request body.
            return Err(illegal(format!(
                // ALLOC: one message.
                "rollover target [{name}] is an index; roll over an alias or a data stream"
            )));
        }
        Target::Missing => {
            return Err(Refusal::new(
                404,
                "index_not_found_exception",
                // ALLOC: one message.
                format!("no such alias or data stream: {name}"),
            ));
        }
        Target::Alias(facts) => facts,
    };
    if facts.mutation_pending {
        return Err(Refusal::new(
            409,
            "version_conflict_engine_exception",
            // ALLOC: one message.
            format!("alias is being changed: {name}"),
        ));
    }
    if facts.data_stream && (new_index.is_some() || shapes_index) {
        return Err(illegal(
            if new_index.is_some() {
                "new index name may not be specified when rolling over a data stream"
            } else {
                "aliases, mappings, and index settings may not be specified when rolling over a data stream"
            }
            // ALLOC: one short string or list, bounded by the request body.
            .to_owned(),
        ));
    }
    if facts.write_index.is_none() {
        // ALLOC: one message.
        return Err(illegal(format!(
            "rollover target [{name}] does not point to a write index"
        )));
    }
    Ok(())
}

/// What the conditions say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    /// Each condition's key and whether it is met, in order.
    pub met: Vec<(String, bool)>,
    /// Whether to roll: with no conditions always, else when any is met.
    pub should_roll: bool,
}

/// Evaluates `conditions` against the current index `index`, created at
/// `created_at_millis` (0 when not recorded), at `now_millis`; `doc_count`
/// is asked for once for each `max_docs` condition.
///
/// # Errors
///
/// A [`Refusal`] for `max_age` on an index whose creation time was not
/// recorded, and whatever `doc_count` refuses.
pub fn evaluate(
    conditions: &[Condition],
    index: &str,
    created_at_millis: i64,
    now_millis: i64,
    doc_count: &mut dyn FnMut() -> Result<u64, Refusal>,
) -> Result<Evaluation, Refusal> {
    // ALLOC: one entry per condition, which the body bounds.
    let mut met = Vec::with_capacity(conditions.len());
    for condition in conditions {
        match condition {
            Condition::MaxAge(age) => {
                if created_at_millis == 0 {
                    return Err(Refusal::new(
                        501,
                        "unsupported_condition",
                        // ALLOC: one message.
                        format!(
                            "max_age cannot be evaluated: [{index}] predates creation times being recorded"
                        ),
                    ));
                }
                let old_enough = now_millis.saturating_sub(created_at_millis) >= age.millis();
                // ALLOC: one key.
                met.push((format!("[max_age: {}]", age.human()), old_enough));
            }
            Condition::MaxDocs(wanted) => {
                let count = doc_count()?;
                // ALLOC: one key.
                met.push((
                    format!("[max_docs: {wanted}]"),
                    i128::from(count) >= i128::from(*wanted),
                ));
            }
        }
    }
    let should_roll = conditions.is_empty() || met.iter().any(|(_, ok)| *ok);
    Ok(Evaluation { met, should_roll })
}
