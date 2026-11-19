//! What a cluster settings call may say and how it answers.
//!
//! The register holds persistent settings only, so a transient setting, a
//! list and a value no node could act on are refused before the register is
//! written. The checks are those the shell makes in the handler, in its order;
//! the plugins' own check is a closure the caller gives, run last.

use std::collections::BTreeMap;

use oss_doc::mapping::json::{Json, parse as parse_json, write_str};

use crate::route::resolve::Refusal;

/// The cluster default of how segments are placed.
pub const SEGMENT_PLACEMENT_KEY: &str = "serverless.segment_placement";
/// The cluster default of the bucket policy.
pub const BUCKET_KEY: &str = "serverless.bucket";
/// The cluster default of the tail overlay.
pub const TAIL_OVERLAY_KEY: &str = "serverless.tail_overlay";
/// The cluster default of refresh visibility.
pub const REFRESH_VISIBILITY_KEY: &str = "serverless.refresh_visibility";
/// How old a staged commit may get before the register must hold it.
pub const CHECKPOINT_KEY: &str = "serverless.wal.node_log.commits.checkpoint_millis";

/// The least a checkpoint age may be, whatever the commit period.
const MIN_CHECKPOINT_MILLIS: u64 = 1_000;
/// How many commit periods a checkpoint age must span at least.
const MIN_COMMIT_PERIODS: u64 = 2;
/// The pack deadline the checkpoint age is bounded by.
const SWAP_DEADLINE_MILLIS: u64 = 5 * 60 * 1000;
/// The most a checkpoint age may be: half the pack deadline.
const MAX_CHECKPOINT_MILLIS: u64 = SWAP_DEADLINE_MILLIS / 2;
/// How many levels a settings body nests before the rest of a name is flat.
const MAX_DEPTH: usize = 64;

/// One setting: its key and its value, none for a removal.
pub type Setting = (String, Option<String>);

/// A setting as the register holds it, for a body to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    /// A single value.
    Value(String),
    /// A list of values.
    List(Vec<String>),
}

fn illegal(reason: &'static str) -> Refusal {
    Refusal::new(400, "illegal_argument_exception", reason)
}

fn refuse_value(reason: impl Into<String>) -> Refusal {
    Refusal::new(400, "status_exception", reason)
}

/// Reads an update body into flat persistent settings, sorted by key, and
/// checks them before anything is written.
///
/// `commit_period_millis` is the node's commit period, which bounds the
/// checkpoint age; `plugins` is the plugins' check of the flat settings.
///
/// # Errors
///
/// A [`Refusal`] for a missing or malformed body, nothing to update, a
/// transient setting, a list, a value one of the named settings refuses, or
/// what `plugins` refuses.
pub fn parse_update(
    body: Option<&[u8]>,
    commit_period_millis: u64,
    plugins: &dyn Fn(&[Setting]) -> Result<(), String>,
) -> Result<Vec<Setting>, Refusal> {
    const NEEDS_PERSISTENT: &str = "a settings update needs a 'persistent' object";
    let body = body
        .filter(|b| !b.is_empty())
        .ok_or_else(|| illegal(NEEDS_PERSISTENT))?;
    let members = match parse_json(body) {
        Ok(Json::Obj(members)) => members,
        Ok(_) => {
            return Err(Refusal::new(
                400,
                "x_content_parse_exception",
                "could not parse the settings body: the body is not a JSON object",
            ));
        }
        Err(e) => {
            return Err(Refusal::new(
                400,
                "x_content_parse_exception",
                // ALLOC: one message, bounded by the parse error's text.
                format!("could not parse the settings body: {e}"),
            ));
        }
    };
    let member = |name: &str| {
        members
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    };
    let Some(Json::Obj(persistent)) = member("persistent") else {
        return Err(illegal(NEEDS_PERSISTENT));
    };
    let transient = match member("transient") {
        None => &[][..],
        Some(Json::Obj(transient)) => transient.as_slice(),
        Some(_) => return Err(illegal("'transient' settings must be an object")),
    };
    let persistent = flatten(persistent);
    let transient = flatten(transient);
    if persistent.is_empty() && transient.is_empty() {
        return Err(Refusal::new(
            400,
            "action_request_validation_exception",
            "Validation Failed: 1: no settings to update;",
        ));
    }
    if !transient.is_empty() {
        return Err(Refusal::new(
            501,
            "status_exception",
            "transient settings are not supported; use persistent",
        ));
    }
    if let Some((key, _)) = persistent
        .iter()
        .find(|(_, leaf)| matches!(leaf, Some(Leaf::List)))
    {
        return Err(Refusal::new(
            501,
            "status_exception",
            // ALLOC: one message, once.
            format!("'{key}' is a list; list-valued settings are not supported on this register"),
        ));
    }
    // ALLOC: one entry a setting of the body, which the edge bounds.
    let flat: Vec<Setting> = persistent
        .into_iter()
        .map(|(key, value)| (key, value.and_then(Leaf::into_text)))
        // ALLOC: the entries moved, none copied.
        .collect();
    check_values(&flat, commit_period_millis)?;
    plugins(&flat).map_err(refuse_value)?;
    Ok(flat)
}

/// What a flattened leaf is.
enum Leaf {
    Text(String),
    List,
}

impl Leaf {
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => Some(text),
            Self::List => None,
        }
    }
}

/// The keys the body names, each with its leaf, none for a null.
type Flat = Vec<(String, Option<Leaf>)>;

/// Flattens an object into dotted keys sorted by key; a later key replaces an
/// earlier one of the same name, and an empty object names nothing.
fn flatten(members: &[(String, Json)]) -> Flat {
    // ALLOC: one entry a leaf of the body, which the edge bounds.
    let mut flat: BTreeMap<String, Option<Leaf>> = BTreeMap::new();
    for (key, value) in members {
        walk(key, value, &mut flat);
    }
    // ALLOC: the entries as a list.
    flat.into_iter().collect()
}

fn walk(key: &str, value: &Json, flat: &mut BTreeMap<String, Option<Leaf>>) {
    match value {
        Json::Obj(members) => {
            for (name, inner) in members {
                // ALLOC: one dotted key, bounded by the body's bytes.
                walk(&format!("{key}.{name}"), inner, flat);
            }
        }
        Json::Null => {
            // ALLOC: one key a leaf of the body.
            flat.insert(key.to_owned(), None);
        }
        Json::Arr(_) => {
            // ALLOC: one key a leaf of the body.
            flat.insert(key.to_owned(), Some(Leaf::List));
        }
        scalar => {
            // ALLOC: one key and value a leaf of the body.
            flat.insert(key.to_owned(), Some(Leaf::Text(scalar.java_string())));
        }
    }
}

/// The value of `key` when the body sets it to something.
fn value_of<'a>(flat: &'a [Setting], key: &str) -> Option<&'a str> {
    flat.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_deref())
}

fn check_values(flat: &[Setting], commit_period_millis: u64) -> Result<(), Refusal> {
    let one_of = |key: &str, allowed: &[&str], what: &str, empty_ok: bool| {
        let Some(value) = value_of(flat, key) else {
            return Ok(());
        };
        let lower = value.to_lowercase();
        if (empty_ok && lower.is_empty()) || allowed.contains(&lower.as_str()) {
            return Ok(());
        }
        // ALLOC: one message, once.
        Err(refuse_value(format!("{what}, got [{value}]")))
    };
    one_of(
        BUCKET_KEY,
        &["auto", "off"],
        "bucket must be [auto] or [off]",
        true,
    )?;
    one_of(
        SEGMENT_PLACEMENT_KEY,
        &["segment_blob", "node_pack"],
        "segment placement must be [segment_blob] or [node_pack]",
        true,
    )?;
    one_of(
        TAIL_OVERLAY_KEY,
        &["true", "false"],
        "tail overlay must be [true] or [false]",
        true,
    )?;
    one_of(
        REFRESH_VISIBILITY_KEY,
        &["owner", "serving", "everywhere"],
        "refresh visibility must be one of [owner, serving, everywhere]",
        false,
    )?;
    if let Some(value) = value_of(flat, CHECKPOINT_KEY) {
        check_checkpoint(value, commit_period_millis)?;
    }
    Ok(())
}

fn check_checkpoint(value: &str, period: u64) -> Result<(), Refusal> {
    let Ok(millis) = value.trim().parse::<i64>() else {
        // ALLOC: one message, once.
        return Err(refuse_value(format!(
            // ALLOC: one message, once.
            "[{CHECKPOINT_KEY}] must be a number of milliseconds, got [{value}]"
        )));
    };
    let least = MIN_CHECKPOINT_MILLIS.max(MIN_COMMIT_PERIODS.saturating_mul(period));
    let deadline = SWAP_DEADLINE_MILLIS;
    let invalid = if least > MAX_CHECKPOINT_MILLIS {
        // ALLOC: one message, once.
        format!(
            "the commit period of {period} ms leaves no checkpoint age: it must be at least \
             {MIN_COMMIT_PERIODS} commit periods and at most {MAX_CHECKPOINT_MILLIS} ms (half the \
             {deadline} ms pack deadline)"
        )
    } else if u64::try_from(millis).map_or(true, |m| m < least || m > MAX_CHECKPOINT_MILLIS) {
        // ALLOC: one message, once.
        format!(
            "the checkpoint age must be between {least} ms ({MIN_COMMIT_PERIODS} commit periods \
             of {period} ms, and at least {MIN_CHECKPOINT_MILLIS} ms) and {MAX_CHECKPOINT_MILLIS} \
             ms (half the {deadline} ms pack deadline), got {millis}"
        )
    } else {
        return Ok(());
    };
    // ALLOC: one message, once.
    Err(refuse_value(format!("[{CHECKPOINT_KEY}]: {invalid}")))
}

/// The body a settings read answers with: the persistent settings, an empty
/// transient block and, when defaults were asked for, an empty defaults one.
#[must_use]
pub fn settings_body(persistent: &[(String, Shown)], flat: bool, defaults: bool) -> String {
    // ALLOC: the body, bounded by the register's size limit.
    let mut out = String::from("{\"persistent\":");
    write_settings(&mut out, persistent, flat);
    out.push_str(",\"transient\":{}");
    if defaults {
        out.push_str(",\"defaults\":{}");
    }
    out.push('}');
    out
}

/// The body a settings update answers with.
#[must_use]
pub fn ack_body(written: &[(String, Shown)], flat: bool) -> String {
    // ALLOC: the body, bounded by the register's size limit.
    let mut out = String::from("{\"acknowledged\":true,\"persistent\":");
    write_settings(&mut out, written, flat);
    out.push_str(",\"transient\":{}}");
    out
}

fn write_settings(out: &mut String, settings: &[(String, Shown)], flat: bool) {
    // ALLOC: one reference a setting, which the register bounds.
    let mut sorted: Vec<(&str, &Shown)> = settings.iter().map(|(k, v)| (k.as_str(), v)).collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    out.push('{');
    if flat {
        let mut first = true;
        for (key, value) in sorted {
            field(out, &mut first, key, value);
        }
    } else {
        nest(out, &sorted, 0);
    }
    out.push('}');
}

fn field(out: &mut String, first: &mut bool, key: &str, value: &Shown) {
    if !*first {
        out.push(',');
    }
    *first = false;
    write_str(out, key);
    out.push(':');
    match value {
        Shown::Value(text) => write_str(out, text),
        Shown::List(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, item);
            }
            out.push(']');
        }
    }
}

/// Writes the members of one level of the tree: the keys grouped by their
/// first name, each group an object, unless a key is also the start of
/// another, or the depth is spent, when the full names are written flat.
fn nest(out: &mut String, entries: &[(&str, &Shown)], depth: usize) {
    // ALLOC: one group a first name, bounded by the entries.
    let mut groups: BTreeMap<&str, Vec<(&str, &Shown)>> = BTreeMap::new();
    for &(key, value) in entries {
        let head = match key.split_once('.') {
            Some((head, _)) if depth < MAX_DEPTH => head,
            _ => key,
        };
        // ALLOC: as above.
        groups.entry(head).or_default().push((key, value));
    }
    let mut first = true;
    for (head, group) in groups {
        let exact = group.iter().any(|(key, _)| *key == head);
        if exact {
            for (key, value) in group {
                field(out, &mut first, key, value);
            }
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        write_str(out, head);
        out.push_str(":{");
        // ALLOC: one reference an entry of the group.
        let inner: Vec<(&str, &Shown)> = group
            .iter()
            .map(|(key, value)| (key.split_once('.').map_or(*key, |(_, rest)| rest), *value))
            // ALLOC: as above.
            .collect();
        nest(out, &inner, depth.saturating_add(1));
        out.push('}');
    }
}
