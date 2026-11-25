//! The mark a restore puts on an index's settings until every shard has
//! landed, so an index a crash left part-way is told apart from a whole one.
//!
//! The mark is one flat setting, written first in the settings object and
//! removed by writing the index's own settings back, so a restored index
//! carries nothing of it.

use crate::cluster_settings::ClusterSettings;
use crate::error::FormatError;

/// The setting a restore holds on an index while it copies.
pub const RESTORING_KEY: &str = "index.restore.in_progress";

/// What an index that carries the mark answers instead of its data.
pub const RESTORING_WHY: &str = "the index is being restored, or a restore of it was interrupted: delete it and restore it again";

/// `settings`, an object or none, with the mark as its first member.
///
/// # Errors
///
/// [`FormatError::Corrupt`] when `settings` is not an object.
pub fn mark_restoring(settings: Option<&[u8]>) -> Result<Vec<u8>, FormatError> {
    // ALLOC: one short string a restore.
    let mark = format!("{{\"{RESTORING_KEY}\":\"true\"");
    let Some(bytes) = settings else {
        // ALLOC: one settings object a restored index.
        return Ok([mark.as_bytes(), b"}"].concat());
    };
    let text = std::str::from_utf8(bytes)
        .map_err(|_| FormatError::Corrupt {
            what: "index settings",
            reason: "settings are not text",
        })?
        .trim();
    let Some(rest) = text.strip_prefix('{') else {
        return Err(FormatError::Corrupt {
            what: "index settings",
            reason: "settings are not an object",
        });
    };
    // ALLOC: one settings object a restored index.
    let comma = if rest.trim_start().starts_with('}') {
        ""
    } else {
        ","
    };
    Ok([mark.as_str(), comma, rest].concat().into_bytes())
}

/// Whether `settings` carry the mark.
#[must_use]
pub fn is_restoring(settings: &[u8]) -> bool {
    match ClusterSettings::parse(settings) {
        Ok(parsed) => parsed.get(RESTORING_KEY).is_some(),
        // Settings that do not parse but name the mark are marked: a corrupt
        // descriptor does not open an index a restore has not finished.
        Err(_) => settings
            .windows(RESTORING_KEY.len())
            .any(|w| w == RESTORING_KEY.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_goes_first_and_the_rest_is_kept() {
        let marked = mark_restoring(Some(br#"{"index.refresh_interval":"5s"}"#)).unwrap();
        assert!(is_restoring(&marked));
        let text = String::from_utf8(marked).unwrap();
        assert!(
            text.ends_with(r#","index.refresh_interval":"5s"}"#),
            "{text}"
        );
    }

    #[test]
    fn empty_or_absent_settings_are_marked_too() {
        for given in [None, Some(&b"{}"[..]), Some(&b"{ }"[..])] {
            let marked = mark_restoring(given).unwrap();
            assert!(is_restoring(&marked), "{marked:?}");
        }
    }

    #[test]
    fn settings_that_are_no_object_are_refused_and_unmarked_ones_read_as_whole() {
        assert!(mark_restoring(Some(b"[1]")).is_err());
        assert!(mark_restoring(Some(&[0xff])).is_err());
        assert!(!is_restoring(br#"{"index.refresh_interval":"5s"}"#));
        assert!(!is_restoring(b"{}"));
        assert!(!is_restoring(b"not json"));
        assert!(!is_restoring(
            b"corrupt settings that are longer than the key and name no mark"
        ));
        assert!(
            is_restoring(br#"{"index.restore.in_progress":"true""#),
            "corrupt but marked"
        );
    }
}
