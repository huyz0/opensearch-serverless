//! The `OSS_PLUGINS` variable: the policy that names the plugins a node loads.
//! No policy loads nothing.

use super::NodeConfig;

/// What a node loads of the plugin system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginPolicy {
    /// The policy text; empty names no plugin.
    pub policy: String,
}

/// Reads the policy with `or`.
pub(in crate::boot) fn read(or: &dyn Fn(&str, &str) -> String) -> PluginPolicy {
    PluginPolicy {
        policy: or("OSS_PLUGINS", ""),
    }
}

/// Gives the assembled node what this node's variables set.
pub(in crate::boot) fn apply(from: &NodeConfig, to: &mut crate::assemble::Config) {
    to.plugin_policy.clone_from(&from.plugin_policy.policy);
}

#[cfg(test)]
mod tests {
    use super::super::NodeConfig;
    use super::super::tests::{lookup, with};

    fn policy(extra: &[(&str, &str)]) -> String {
        NodeConfig::from_lookup(lookup(&with(extra)))
            .expect("valid")
            .plugin_policy
            .policy
    }

    #[test]
    fn no_variable_names_no_plugin() {
        assert_eq!(policy(&[]), "");
    }

    #[test]
    fn the_variable_carries_the_policy_text() {
        let text = r#"{"plugins":[]} "#;
        assert_eq!(policy(&[("OSS_PLUGINS", text)]), text);
    }
}
