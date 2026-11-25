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
    for (key, why) in REFUSED {
        if members.iter().any(|(k, _)| k == key) {
            return Err(refuse(
                501,
                "unsupported_restore",
                // ALLOC: error path; one message.
                format!("{key} is not supported: {why}"),
            ));
        }
    }
    if let Some((key, _)) = members.iter().find(|(k, _)| !KNOWN.contains(&k.as_str())) {
        return Err(refuse(
            400,
            "unknown_parameter",
            // ALLOC: error path; one message.
            format!("Unknown parameter {key}"),
        ));
    }
    Ok(())
}
