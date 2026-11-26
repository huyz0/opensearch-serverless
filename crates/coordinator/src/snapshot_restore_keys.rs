//! The keys of a restore's body: the eight it reads, the nine it
//! refuses with a reason, and the rest it does not know
//! (`SnapshotRequestBodies.RESTORE_KNOWN_KEYS` and `RESTORE_REFUSED_KEYS`).

use oss_doc::mapping::json::Json;

use crate::route::resolve::Refusal;

/// What a restore reads; the rest is refused below.
const KNOWN: [&str; 8] = [
    "partial",
    "indices",
    "rename_pattern",
    "rename_replacement",
    "index_settings",
    "ignore_index_settings",
    "rename_alias_pattern",
    "rename_alias_replacement",
];

/// Core's other restore fields, each with why it is refused: the shell's
/// `RESTORE_REFUSED_KEYS`.
pub(crate) const REFUSED: [(&str, &str); 9] = [
    (
        "include_global_state",
        "there is no global state in a snapshot here to restore",
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
        "a restore adds the aliases the snapshot recorded to the indices it creates; it does not choose who keeps the write index",
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

/// Boolean flags: `partial` and `include_aliases` are read; the rest name behaviour this
/// node always has, so `false` changes nothing and `true` stays refused.
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
