//! One alias change: add or remove a member, retried while another writer
//! swaps the alias first.

use std::borrow::Cow;

use oss_format::AliasRecord;
use oss_meta::{AliasMutations, MetaError};
use oss_rest::RestError;

use super::parse::{Change, refuse};

/// Times a change is retried when another writer swapped the alias first.
const ATTEMPTS: usize = 4;

fn meta_failed(e: &MetaError, change: &Change) -> RestError {
    match e {
        MetaError::MissingMember(name) => refuse(
            404,
            "index_not_found_exception",
            format!("no such index [{name}]"),
        ),
        MetaError::AlreadyExists => refuse(
            400,
            "invalid_alias_name_exception",
            format!(
                "Invalid alias name [{}]: an index or alias with the same name already exists",
                change.alias
            ),
        ),
        MetaError::Superseded | MetaError::Contended => refuse(
            409,
            "alias_contended_exception",
            format!(
                "alias [{}] changed while it was being updated",
                change.alias
            ),
        ),
        other => refuse(500, "alias_update_failure", other.to_string()),
    }
}

/// Adds `index` to `alias`, as an `add` action of `POST /_aliases` does.
pub(crate) fn add_alias(
    m: &mut AliasMutations<'_>,
    index: &str,
    alias: &str,
    write: Option<bool>,
) -> Result<(), RestError> {
    apply(
        m,
        &Change {
            add: true,
            index: index.to_owned(),
            alias: alias.to_owned(),
            write,
        },
    )
}

/// Applies one change, retrying when another writer swapped the alias first.
pub(super) fn apply(m: &mut AliasMutations<'_>, change: &Change) -> Result<(), RestError> {
    for _ in 0..ATTEMPTS {
        if step(m, change)? {
            return Ok(());
        }
    }
    Err(refuse(
        409,
        "alias_contended_exception",
        format!("alias [{}] kept changing", change.alias),
    ))
}

/// One attempt; `false` when the alias moved under it.
fn step(m: &mut AliasMutations<'_>, change: &Change) -> Result<bool, RestError> {
    let failed = |e: MetaError| meta_failed(&e, change);
    let index = change.index.as_str();
    let Some(at) = m.resolve_for_mutation(&change.alias).map_err(failed)? else {
        if !change.add {
            return Err(not_found(change));
        }
        let mut record = AliasRecord::new(&change.alias, &[index]);
        if change.write == Some(true) {
            record.write_member = Some(Cow::Borrowed(index));
        }
        return match m.create_alias(&record) {
            Ok(_) => Ok(true),
            Err(MetaError::Superseded) => Ok(false),
            Err(e) => Err(failed(e)),
        };
    };
    let before = at.alias;
    if before.data_stream {
        return Err(refuse(
            400,
            "invalid_alias_name_exception",
            format!("[{}] is a data stream, not an alias", change.alias),
        ));
    }
    let member = before.indices.iter().any(|i| i == index);
    let mut after = before.clone();
    if change.add {
        if !member {
            after.indices.push(Cow::Owned(change.index.clone()));
        }
        match change.write {
            Some(true) => after.write_member = Some(Cow::Owned(change.index.clone())),
            Some(false) if before.write_index() == Some(index) => after.write_member = None,
            _ => {}
        }
        if after == before {
            return Ok(true);
        }
    } else {
        if !member {
            return Err(not_found(change));
        }
        after.indices.retain(|i| i != index);
        if before.write_index() == Some(index) {
            after.write_member = None;
        }
        if after.indices.is_empty() {
            return m.delete_alias(&before, at.generation).map_err(failed);
        }
    }
    Ok(m.update_alias(&before, &after, at.generation)
        .map_err(failed)?
        .is_some())
}

fn not_found(change: &Change) -> RestError {
    refuse(
        404,
        "aliases_not_found_exception",
        format!("aliases [{}] missing", change.alias),
    )
}
