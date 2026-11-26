//! Which aliases a restore adds, planned before anything is written (ADR
//! 0065): the recorded aliases of the restored indices, renamed and grouped
//! by name.

use oss_doc::mapping::json::write_str;

use crate::route::resolve::Refusal;
use crate::snapshot_restore_admin::{RestoreRequest, Target};
use crate::snapshot_take_admin::too_large;

/// The most distinct aliases one restore adds: a take's bound, 100 indices
/// of 64 aliases each.
pub const MAX_RESTORE_ALIASES: usize = 6_400;

/// One alias a restore adds: its name after the rename and each restored
/// index that joins it with whether the snapshot made it the write index,
/// in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasPlan {
    /// The alias's name.
    pub name: String,
    /// The restored indices (target names) and their write flags.
    pub members: Vec<(String, bool)>,
}

/// Groups the aliases of the restored indices by alias name, renamed, in the
/// order they are met. `recorded[i]` is what the snapshot recorded for
/// `targets[i]`. Nothing is planned without `include_aliases`.
///
/// # Errors
///
/// 400 `invalid_alias_name_exception` for a name that is empty, over 255
/// bytes, starts with `_` or is also a restored index; 400
/// `snapshot_too_large` for more than [`MAX_RESTORE_ALIASES`] aliases.
pub fn plan_aliases(
    request: &RestoreRequest,
    targets: &[Target],
    recorded: &[&[oss_format::AliasCapture]],
) -> Result<Vec<AliasPlan>, Refusal> {
    if !request.include_aliases {
        // ALLOC: none held; an empty plan.
        return Ok(Vec::new());
    }
    // ALLOC: one entry an alias, at most MAX_RESTORE_ALIASES.
    let mut plans: Vec<AliasPlan> = Vec::new();
    let invalid = |name: &str, why: &str| {
        Refusal::new(
            400,
            "invalid_alias_name_exception",
            // ALLOC: error path; one message.
            format!("Invalid alias name [{name}]: {why}"),
        )
    };
    for (target, aliases) in targets.iter().zip(recorded) {
        for captured in *aliases {
            let name = request
                .alias_rename
                .as_ref()
                .map_or_else(|| captured.name.clone(), |r| r.apply(&captured.name));
            if name.is_empty() || name.len() > oss_format::MAX_ALIAS_NAME_BYTES {
                return Err(invalid(&name, "a name is 1 to 255 bytes"));
            }
            if name.starts_with('_') {
                return Err(invalid(&name, "a name must not start with '_'"));
            }
            if targets.iter().any(|t| t.target == name) {
                return Err(invalid(&name, "an index with the same name is restored"));
            }
            let member = (target.target.clone(), captured.write);
            if let Some(plan) = plans.iter_mut().find(|p| p.name == name) {
                if !plan.members.iter().any(|(t, _)| *t == member.0) {
                    plan.members.push(member);
                }
                continue;
            }
            if plans.len() >= MAX_RESTORE_ALIASES {
                // ALLOC: error path; one message.
                return Err(too_large(&format!(
                    "a restore adds at most {MAX_RESTORE_ALIASES} aliases"
                )));
            }
            plans.push(AliasPlan {
                name,
                // ALLOC: one member, the alias's first.
                members: vec![member],
            });
        }
    }
    Ok(plans)
}

/// What a restore asked for aliases did, for its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasReport {
    /// How many distinct aliases were added to.
    pub restored: usize,
    /// The aliases that could not be added.
    pub failed: Vec<String>,
    /// The restored indices whose record was cut at the alias bound.
    pub truncated: Vec<String>,
}

impl AliasReport {
    /// Appends the members this report adds to the `snapshot` object.
    pub(crate) fn write(&self, out: &mut String) {
        use std::fmt::Write;
        let _ = write!(out, ",\"aliases_restored\":{}", self.restored);
        for (key, names) in [
            ("failed_aliases", &self.failed),
            ("aliases_truncated", &self.truncated),
        ] {
            if names.is_empty() {
                continue;
            }
            let _ = write!(out, ",\"{key}\":[");
            for (i, name) in names.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(out, name);
            }
            out.push(']');
        }
    }
}
