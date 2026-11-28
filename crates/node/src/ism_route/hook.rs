//! The create hook of `ism_template`: attaches the policy whose template
//! matches a new index.

use oss_rest::{Request, RestError};

use super::IsmService;
use super::templates::Entry;

/// Whether `name` fits `pattern`, where `*` stands for any run of characters.
pub(super) fn glob(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return false;
    };
    if parts.len() == 1 {
        return pattern == name;
    }
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for mid in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        match rest.find(mid) {
            Some(at) => rest = rest.get(at.saturating_add(mid.len())..).unwrap_or_default(),
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// The policy a new index `name` is attached to: the highest priority, the
/// smaller id on a tie.
pub(super) fn matching<'a>(entries: &'a [Entry], name: &str) -> Option<&'a str> {
    entries
        .iter()
        .filter(|e| e.patterns.iter().any(|p| glob(p, name)))
        .min_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.policy_id.cmp(&b.policy_id))
        })
        .map(|e| e.policy_id.as_str())
}

impl IsmService {
    /// The hook an index create calls: attaches the policy whose template
    /// matches the new index, if any. It never fails the create.
    #[must_use]
    pub fn on_create(self: &std::sync::Arc<Self>) -> crate::index_route::Created {
        let service = std::sync::Arc::clone(self);
        std::sync::Arc::new(move |req, index, uuid, created| {
            let _ = service.attach_matching(req, index, uuid, created);
        })
    }

    fn attach_matching(
        &self,
        req: &mut Request<'_>,
        index: &str,
        uuid: &str,
        created: i64,
    ) -> Result<(), RestError> {
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let entries = self.all_templates(&mut marker, &mut doc)?;
        if let Some(policy) = matching(&entries, index) {
            self.attach(index, uuid, created, policy, None, &mut marker, &mut doc)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_matches_with_stars_only() {
        assert!(glob("logs-*", "logs-1"));
        assert!(glob("*", "x"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(glob("a*b*c", "abc"));
        assert!(!glob("a*b*c", "acb"));
        assert!(!glob("logs-*", "log"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exact2"));
        assert!(!glob("a*a", "a"));
    }

    #[test]
    fn the_highest_priority_wins_and_a_tie_goes_to_the_smaller_id() {
        let e = |id: &str, priority, p: &str| Entry {
            policy_id: id.to_owned(),
            priority,
            patterns: vec![p.to_owned()],
        };
        let all = [e("b", 1, "x*"), e("a", 1, "x*"), e("c", 0, "x*")];
        assert_eq!(matching(&all, "xy"), Some("a"));
        assert_eq!(matching(&all, "y"), None);
    }
}
