//! `ism_template`: the patterns a policy attaches itself to new indices by.
//!
//! Every policy's templates live in one aggregate document of the
//! `ism/templates/` set, so an index create reads one document and never a
//! policy per candidate. A policy put or delete changes the aggregate after
//! its own write and re-reads it to confirm the change survived a racing
//! writer, so no entry is lost for good.

use oss_format::DocKind;
use oss_rest::RestError;
use serde_json::{Value, json};

use super::{IsmService, bad, failed, refuse};

/// The aggregate's name in its set.
const ALL: &str = "all";
/// Templates one policy may carry, and patterns one template may carry.
const MAX_TEMPLATES: usize = 5;
const MAX_PATTERNS: usize = 10;
/// Writes of the aggregate before a lost race is reported.
const ATTEMPTS: usize = 5;

/// One template of one policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) policy_id: String,
    pub(super) priority: i64,
    pub(super) patterns: Vec<String>,
}

impl Entry {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            policy_id: v.get("policy_id")?.as_str()?.to_owned(),
            priority: v.get("priority")?.as_i64()?,
            patterns: v
                .get("index_patterns")?
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect(),
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "policy_id": self.policy_id,
            "priority": self.priority,
            "index_patterns": self.patterns,
        })
    }
}

/// The templates a policy body carries, checked.
///
/// # Errors
///
/// The reason a template is refused, for a 400.
pub(super) fn entries_of(id: &str, policy: &Value) -> Result<Vec<Entry>, String> {
    let one = |t: &Value| -> Result<Entry, String> {
        let m = t
            .as_object()
            .ok_or("[ism_template] must be an object or an array of objects")?;
        let patterns: Vec<String> = m
            .get("index_patterns")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|p| p.as_str().map(str::to_owned))
                    .collect::<Option<_>>()
            })
            .unwrap_or(None)
            .filter(|p: &Vec<String>| !p.is_empty() && p.len() <= MAX_PATTERNS)
            .ok_or_else(|| format!("[index_patterns] needs 1 to {MAX_PATTERNS} strings"))?;
        if patterns.iter().any(|p| p.is_empty() || p.starts_with('-')) {
            return Err("an index pattern is not empty and does not start with [-]".to_owned());
        }
        let priority = m
            .get("priority")
            .and_then(Value::as_i64)
            .ok_or("[priority] must be an integer")?;
        Ok(Entry {
            policy_id: id.to_owned(),
            priority,
            patterns,
        })
    };
    let list = match policy.get("ism_template") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() <= MAX_TEMPLATES => a.iter().collect::<Vec<_>>(),
        Some(Value::Array(_)) => {
            return Err(format!(
                "a policy carries at most {MAX_TEMPLATES} templates"
            ));
        }
        Some(other) => vec![other],
    };
    list.into_iter().map(one).collect()
}

/// Refuses `wanted` when another policy holds the same pattern at the same
/// priority, which would leave the winner to chance.
fn overlap(wanted: &[Entry], current: &[Entry]) -> Result<(), String> {
    for new in wanted {
        for old in current
            .iter()
            .filter(|e| e.policy_id != new.policy_id && e.priority == new.priority)
        {
            if let Some(p) = new.patterns.iter().find(|p| old.patterns.contains(p)) {
                return Err(format!(
                    "index pattern [{p}] at priority [{}] overlaps policy [{}]",
                    new.priority, old.policy_id
                ));
            }
        }
    }
    Ok(())
}

impl IsmService {
    /// The aggregate's entries; none when no policy has a template.
    pub(super) fn all_templates(
        &self,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Vec<Entry>, RestError> {
        let stored = self.stored_in(DocKind::IsmTemplates, ALL, marker, doc)?;
        Ok(stored
            .as_ref()
            .and_then(|s| s["templates"].as_array())
            .map(|a| a.iter().filter_map(Entry::from_json).collect())
            .unwrap_or_default())
    }

    /// Checks `wanted` against the other policies' templates before the
    /// policy is written; the entries now held, for [`Self::sync_templates`].
    ///
    /// # Errors
    ///
    /// 400 for an overlap, and a failed store call.
    pub(super) fn check_templates(
        &self,
        wanted: &[Entry],
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Vec<Entry>, RestError> {
        let current = self.all_templates(marker, doc)?;
        overlap(wanted, &current).map_err(bad)?;
        Ok(current)
    }

    /// Makes the aggregate hold exactly `wanted` for policy `id`, and
    /// confirms it by reading it back.
    ///
    /// # Errors
    ///
    /// 409 when a racing writer keeps undoing the change, and a failed
    /// store call.
    pub(super) fn sync_templates(
        &self,
        id: &str,
        wanted: &[Entry],
        mut current: Vec<Entry>,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<(), RestError> {
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                current = self.all_templates(marker, doc)?;
            }
            let mine: Vec<&Entry> = current.iter().filter(|e| e.policy_id == id).collect();
            if mine.len() == wanted.len() && mine.iter().zip(wanted).all(|(a, b)| *a == b) {
                return Ok(());
            }
            let next: Vec<Value> = current
                .iter()
                .filter(|e| e.policy_id != id)
                .chain(wanted)
                .map(Entry::to_json)
                .collect();
            // ALLOC: the aggregate's text, bounded by the kind and checked by the put.
            let bytes = json!({"templates": next}).to_string().into_bytes();
            self.set_of(DocKind::IsmTemplates)?
                .put(ALL, &bytes, false, marker)
                .map_err(|e| failed(&e))?;
            current = self.all_templates(marker, doc)?;
            let held: Vec<&Entry> = current.iter().filter(|e| e.policy_id == id).collect();
            if held.len() == wanted.len() && held.iter().zip(wanted).all(|(a, b)| *a == b) {
                return Ok(());
            }
        }
        Err(refuse(
            409,
            "version_conflict_engine_exception",
            "the templates are changing; try again",
        ))
    }
}
