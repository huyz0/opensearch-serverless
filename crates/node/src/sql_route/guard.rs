//! The permission check a SQL statement's index names go through.

use std::sync::Arc;

use oss_coordinator::security::{Who, forbidden};

use super::SqlGuard;
use crate::security_state::SecurityState;

/// The check the SQL handler makes of each index name its statement names.
pub struct NameGuard {
    state: Arc<SecurityState>,
}

impl NameGuard {
    /// A guard over `state`.
    #[must_use]
    pub fn new(state: Arc<SecurityState>) -> Self {
        Self { state }
    }
}

impl SqlGuard for NameGuard {
    fn allow(
        &self,
        principal: Option<&oss_rest::Identity>,
        action: &str,
        names: &[&str],
    ) -> Result<(), String> {
        let Ok(config) = self.state.config() else {
            return Err(r#"{"error":{"type":"security_exception","reason":"The security configuration is unavailable"},"status":403}"#.to_owned());
        };
        let who = principal.map_or(Who::stored(""), crate::security_who::who);
        if principal.is_some() && config.indices_allow(who, action, names) {
            Ok(())
        } else {
            Err(forbidden(&config, who, action))
        }
    }
}
