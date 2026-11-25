//! Whether an index is warmed into a joining reader: the index's own
//! setting, under the node's switch.

use oss_types::settings::{BoolSetting, Scope, Setting, Source};

/// `index.serverless.search.warm_on_join.enabled`: whether a node serving this
/// index tells a joining reader of it and serves its pages. Default `true`:
/// the node's switch (`OSS_WARM_ON_JOIN`, off unless set) is what turns warm
/// copy on; an index sets `false` to be left cold.
pub const WARM_INDEX_ENABLED: BoolSetting = BoolSetting::new(
    "index.serverless.search.warm_on_join.enabled",
    true,
    Scope::Index,
    "Let a joining reader take this index's hot pages from the nodes serving it, \
     where the node has warm copy on.",
);

/// Every setting this module defines, for the node's registry.
pub static SETTINGS: &[&Setting] = &[WARM_INDEX_ENABLED.setting()];

/// Whether an index with these settings is warmed: its own setting, and
/// `true` when it has none or one that does not parse.
pub fn index_wants_warm(settings: &dyn Source) -> bool {
    settings
        .value(WARM_INDEX_ENABLED.key())
        .filter(|v| !v.is_empty())
        .and_then(|v| WARM_INDEX_ENABLED.parse(v).ok())
        .unwrap_or_else(|| WARM_INDEX_ENABLED.default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An index's settings: this one key, or none.
    struct Index(Option<&'static str>);

    impl Source for Index {
        fn value(&self, key: &str) -> Option<&str> {
            (key == WARM_INDEX_ENABLED.key())
                .then_some(self.0)
                .flatten()
        }
    }

    // docs/specs/reader-warm-copy AC-2
    #[test]
    fn an_index_is_warmed_unless_it_says_false() {
        assert!(index_wants_warm(&Index(None)));
        assert!(index_wants_warm(&Index(Some("true"))));
        assert!(index_wants_warm(&Index(Some(""))));
        assert!(
            index_wants_warm(&Index(Some("maybe"))),
            "unparsed: the default"
        );
        assert!(!index_wants_warm(&Index(Some("false"))));
    }
}
