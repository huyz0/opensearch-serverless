//! The `OSS_INDEX_TEMPLATES` and `OSS_INGEST_PIPELINES` variables: whether a
//! node serves index templates and ingest pipelines.

use super::NodeConfig;

/// What a node serves of templates, pipelines and notification configs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthoringConfig {
    /// `serverless.index_templates.enabled`: serve the template routes and apply a template at create. Off by default.
    pub index_templates: bool,
    /// `serverless.ingest.pipelines.enabled`: serve the pipeline routes and run the pipeline a write names. Off by default.
    pub ingest_pipelines: bool,
    /// `serverless.search.pipelines.enabled`: serve the search pipeline routes and run the pipeline a search names. Off by default.
    pub search_pipelines: bool,
    /// `serverless.notifications.enabled`: serve the notifications config routes. Off by default.
    pub notifications: bool,
}

/// Reads the switches with `flag`.
pub(in crate::boot) fn read(
    flag: &dyn Fn(&str, &str) -> Result<bool, String>,
) -> Result<AuthoringConfig, String> {
    Ok(AuthoringConfig {
        index_templates: flag("OSS_INDEX_TEMPLATES", "false")?,
        ingest_pipelines: flag("OSS_INGEST_PIPELINES", "false")?,
        search_pipelines: flag("OSS_SEARCH_PIPELINES", "false")?,
        notifications: flag("OSS_NOTIFICATIONS", "false")?,
    })
}

/// Gives the assembled node what this node's variables set.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.index_templates = from.authoring.index_templates;
    to.ingest_pipelines = from.authoring.ingest_pipelines;
    to.search_pipelines = from.authoring.search_pipelines;
    to.notifications = from.authoring.notifications;
}

#[cfg(test)]
mod tests {
    use super::super::NodeConfig;
    use super::super::tests::{lookup, with};

    fn config(extra: &[(&str, &str)]) -> super::AuthoringConfig {
        NodeConfig::from_lookup(lookup(&with(extra)))
            .expect("valid")
            .authoring
    }

    #[test]
    fn templates_and_pipelines_are_off_until_set_and_each_stands_alone() {
        let defaults = config(&[]);
        assert!(!defaults.index_templates && !defaults.ingest_pipelines);
        let templates = config(&[("OSS_INDEX_TEMPLATES", "true")]);
        assert!(templates.index_templates && !templates.ingest_pipelines);
        let pipelines = config(&[("OSS_INGEST_PIPELINES", "true")]);
        assert!(pipelines.ingest_pipelines && !pipelines.index_templates);
        let search = config(&[("OSS_SEARCH_PIPELINES", "true")]);
        assert!(search.search_pipelines && !search.ingest_pipelines && !defaults.search_pipelines);
        for name in [
            "OSS_INDEX_TEMPLATES",
            "OSS_INGEST_PIPELINES",
            "OSS_SEARCH_PIPELINES",
        ] {
            assert!(NodeConfig::from_lookup(lookup(&with(&[(name, "yes")]))).is_err());
        }
    }
}
