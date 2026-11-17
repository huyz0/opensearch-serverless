//! Versions as `VersionType` and `Versions` give them: the special
//! versions, each type's conflict rule, the explanation a conflict
//! carries, and the version a write takes.
//!
//! Ported from `VersionType` and `Versions` (server, OpenSearch 3.10).

/// `Versions.MATCH_ANY`: any version, or none.
pub const MATCH_ANY: i64 = -3;
/// `Versions.NOT_FOUND`: no document has the id.
pub const NOT_FOUND: i64 = -1;
/// `Versions.MATCH_DELETED`: the document must not exist (`op_type=create`).
pub const MATCH_DELETED: i64 = -4;

/// `VersionType`: how a request's version is checked and applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionType {
    /// The engine counts versions; a request may only ask for any version
    /// or for none (`create`).
    Internal,
    /// The client gives versions; a write needs one higher than the
    /// current.
    External,
    /// As `External`, but an equal version is accepted.
    ExternalGte,
}

impl VersionType {
    /// `VersionType.fromString`: `internal`, `external`, `external_gte`;
    /// `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "internal" => Some(VersionType::Internal),
            "external" => Some(VersionType::External),
            "external_gte" => Some(VersionType::ExternalGte),
            _ => None,
        }
    }

    /// `isVersionConflictForWrites`.
    pub fn conflicts(self, current: i64, expected: i64, deleted: bool) -> bool {
        match self {
            VersionType::Internal => match expected {
                MATCH_ANY => false,
                MATCH_DELETED => !deleted,
                _ => current != expected,
            },
            VersionType::External => {
                current != NOT_FOUND && (expected == MATCH_ANY || current >= expected)
            }
            VersionType::ExternalGte => {
                current != NOT_FOUND && (expected == MATCH_ANY || current > expected)
            }
        }
    }

    /// `explainConflictForWrites`, for a conflict [`VersionType::conflicts`]
    /// found.
    pub fn explain(self, current: i64, expected: i64) -> String {
        match self {
            VersionType::Internal if expected == MATCH_DELETED => {
                // ALLOC: one message; refused write.
                format!("document already exists (current version [{current}])")
            }
            VersionType::Internal if current == NOT_FOUND => {
                // ALLOC: one message; refused write.
                format!("document does not exist (expected version [{expected}])")
            }
            VersionType::Internal => {
                // ALLOC: one message; refused write.
                format!(
                    "current version [{current}] is different than the one provided [{expected}]"
                )
            }
            VersionType::External => {
                // ALLOC: one message; refused write.
                format!(
                    "current version [{current}] is higher or equal to the one provided [{expected}]"
                )
            }
            VersionType::ExternalGte => {
                // ALLOC: one message; refused write.
                format!("current version [{current}] is higher than the one provided [{expected}]")
            }
        }
    }

    /// `updateVersion`: the version a write that does not conflict takes.
    pub fn next(self, current: i64, expected: i64) -> i64 {
        match self {
            VersionType::Internal if current == NOT_FOUND => 1,
            VersionType::Internal => current.saturating_add(1),
            VersionType::External | VersionType::ExternalGte => expected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_versions() {
        let t = VersionType::Internal;
        assert!(!t.conflicts(5, MATCH_ANY, false));
        assert!(t.conflicts(5, MATCH_DELETED, false));
        assert!(!t.conflicts(5, MATCH_DELETED, true));
        assert!(t.conflicts(5, 4, false) && !t.conflicts(5, 5, false));
        assert_eq!(t.next(NOT_FOUND, MATCH_ANY), 1);
        assert_eq!(t.next(5, MATCH_ANY), 6);
        assert_eq!(
            t.explain(2, MATCH_DELETED),
            "document already exists (current version [2])"
        );
        assert_eq!(
            t.explain(NOT_FOUND, 3),
            "document does not exist (expected version [3])"
        );
        assert_eq!(
            t.explain(2, 3),
            "current version [2] is different than the one provided [3]"
        );
    }

    #[test]
    fn external_versions() {
        let (e, g) = (VersionType::External, VersionType::ExternalGte);
        for t in [e, g] {
            assert!(!t.conflicts(NOT_FOUND, 1, true));
            assert!(t.conflicts(3, MATCH_ANY, false));
            assert!(t.conflicts(5, 4, false) && !t.conflicts(5, 6, false));
            assert_eq!(t.next(5, 9), 9);
        }
        assert!(e.conflicts(5, 5, false) && !g.conflicts(5, 5, false));
        assert_eq!(
            e.explain(10, 9),
            "current version [10] is higher or equal to the one provided [9]"
        );
        assert_eq!(
            g.explain(10, 9),
            "current version [10] is higher than the one provided [9]"
        );
        assert_eq!(VersionType::parse("external_gte"), Some(g));
        assert_eq!(VersionType::parse("internal"), Some(VersionType::Internal));
        assert_eq!(VersionType::parse("external"), Some(e));
        assert_eq!(VersionType::parse("force"), None);
    }
}
