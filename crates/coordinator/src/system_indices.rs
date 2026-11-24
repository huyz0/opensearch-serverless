//! The index names no request may capture or restore into: patterns an
//! operator declares, in the simple `*` syntax, as the shell's `SystemIndices`
//! holds the ones plugins declare.

use crate::glob::glob;
use crate::route::resolve::Refusal;

/// The declared patterns.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SystemIndices {
    patterns: Vec<String>,
}

impl SystemIndices {
    /// No system index: nothing is refused.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// The patterns in a comma-separated list; blanks are dropped.
    #[must_use]
    pub fn parse(list: &str) -> Self {
        Self {
            patterns: list
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                // ALLOC: one entry a pattern, once a node.
                .map(str::to_owned)
                .collect(),
        }
    }

    /// Whether `index` matches a declared pattern.
    #[must_use]
    pub fn contains(&self, index: &str) -> bool {
        self.patterns.iter().any(|p| glob(p, index))
    }
}

/// The refusal for restoring a system index, or into one.
#[must_use]
pub fn restore_refusal(index: &str) -> Refusal {
    Refusal {
        status: 403,
        kind: "system_index",
        // ALLOC: error path; one message.
        reason: format!("[{index}] is a system index and cannot be restored"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_system_index_when_a_pattern_matches_all_of_it() {
        let s = SystemIndices::parse(" .kibana*, .tasks ,,");
        assert!(s.contains(".kibana"));
        assert!(s.contains(".kibana_1"));
        assert!(s.contains(".tasks"));
        assert!(
            !s.contains(".tasks2"),
            "an exact pattern matches the whole name"
        );
        assert!(!s.contains("logs"));
        assert!(!SystemIndices::none().contains(".kibana"));
        assert!(!SystemIndices::parse("").contains(""));
    }

    #[test]
    fn the_restore_refusal_is_a_403() {
        let r = restore_refusal(".kibana");
        assert_eq!((r.status, r.kind), (403, "system_index"));
        assert!(r.reason.contains(".kibana") && r.reason.contains("restored"));
    }
}
