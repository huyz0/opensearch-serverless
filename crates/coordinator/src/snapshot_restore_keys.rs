//! The keys of a restore's body: the five it reads, the thirteen it
//! refuses with a reason, and the rest it does not know
//! (`SnapshotRequestBodies.RESTORE_KNOWN_KEYS` and `RESTORE_REFUSED_KEYS`).

use oss_doc::mapping::json::Json;

use crate::route::resolve::Refusal;

/// What a restore reads; the rest is refused below.
const KNOWN: [&str; 5] = [
    "indices",
    "rename_pattern",
    "rename_replacement",
    "index_settings",
    "ignore_index_settings",
];

/// Core's other restore fields, each with why it is refused: the shell's
/// `RESTORE_REFUSED_KEYS`.
pub(crate) const REFUSED: [(&str, &str); 13] = [
    (
        "partial",
        "a restore here creates every shard of an index or refuses; there is no partial index to make",
    ),
    (
        "include_global_state",
        "there is no global state in a snapshot here to restore",
    ),
    (
        "include_aliases",
        "aliases are not captured in a snapshot here",
    ),
    (
        "rename_alias_pattern",
        "aliases are not captured in a snapshot here",
    ),
    (
        "rename_alias_replacement",
        "aliases are not captured in a snapshot here",
    ),
    (
        "settings",
        "repository settings are fixed when the repository is registered",
    ),
    (
        "storage_type",
        "a restored index is served from this deployment's own object store; there is no other storage type",
    ),
    (
        "source_remote_store_repository",
        "there is one object store here, and it is the repository",
    ),
    (
        "source_remote_translog_repository",
        "there is one object store here, and it is the repository",
    ),
    (
        "alias_write_index_policy",
        "aliases are not captured in a snapshot here",
    ),
    (
        "ignore_unavailable",
        "a restore names the indices it wants; one the snapshot did not capture is a 404",
    ),
    (
        "allow_no_indices",
        "a restore names the indices it wants; patterns are not matched against the snapshot",
    ),
    (
        "expand_wildcards",
        "a restore names the indices it wants; patterns are not matched against the snapshot",
    ),
];

/// Flags that name behaviour this node always has: set to `false` they
/// change nothing, set to `true` they stay refused below.
const OFF: [&str; 4] = [
    "partial",
    "include_global_state",
    "include_aliases",
    "ignore_unavailable",
];

fn refuse(status: u16, kind: &'static str, reason: String) -> Refusal {
    Refusal {
        status,
        kind,
        reason,
    }
}

/// Refuses the first key the shell refuses, then the first it does not know.
///
/// # Errors
///
/// 501 `unsupported_restore`; 400 `unknown_parameter`.
pub(crate) fn check_keys(members: &[(String, Json)]) -> Result<(), Refusal> {
    for key in OFF {
        match members.iter().find(|(k, _)| k == key) {
            Some((_, Json::Bool(_))) | None => {}
            Some(_) => {
                return Err(refuse(
                    400,
                    "illegal_argument_exception",
                    // ALLOC: error path; one message.
                    format!("{key} must be a boolean"),
                ));
            }
        }
    }
    for (key, why) in REFUSED {
        let at_default = OFF.contains(&key)
            && members
                .iter()
                .any(|(k, v)| k == key && matches!(v, Json::Bool(false)));
        if at_default {
            continue;
        }
        if members.iter().any(|(k, _)| k == key) {
            return Err(refuse(
                501,
                "unsupported_restore",
                // ALLOC: error path; one message.
                format!("{key} is not supported: {why}"),
            ));
        }
    }
    let known = |k: &str| KNOWN.contains(&k) || OFF.contains(&k);
    if let Some((key, _)) = members.iter().find(|(k, _)| !known(k)) {
        return Err(refuse(
            400,
            "unknown_parameter",
            // ALLOC: error path; one message.
            format!("Unknown parameter {key}"),
        ));
    }
    Ok(())
}
