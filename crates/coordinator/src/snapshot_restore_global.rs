//! What a restore asked for the snapshot's global state decides and says
//! (ADR 0068): the refusal for a set that would pass its bound, and the
//! members the answer gains.

use std::fmt::Write;

use oss_doc::mapping::json::write_str;

use crate::route::resolve::Refusal;

/// What a restore asked for the global state did, for its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalReport {
    /// How many documents and settings were applied.
    pub restored: usize,
    /// What could not be applied, as `index_template/<name>`,
    /// `persistent_settings/<key>` and the like.
    pub failed: Vec<String>,
}

impl GlobalReport {
    /// Appends the members this report adds to the `snapshot` object.
    pub(crate) fn write(&self, out: &mut String) {
        let _ = write!(out, ",\"global_state_restored\":{}", self.restored);
        if self.failed.is_empty() {
            return;
        }
        out.push_str(",\"failed_global_state\":[");
        for (i, item) in self.failed.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_str(out, item);
        }
        out.push(']');
    }
}

/// The refusal for a restore after which a set of the cluster would hold
/// more than `limit` (documents, or bytes for the settings): 400, naming
/// the set, before anything is written.
#[must_use]
pub fn global_state_too_large(set: &str, limit: &str) -> Refusal {
    Refusal {
        status: 400,
        kind: "illegal_argument_exception",
        // ALLOC: error path; one message.
        reason: format!(
            "restoring the global state would leave [{set}] with more than {limit}; nothing was restored"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_names_what_was_applied_and_what_failed() {
        let mut out = String::new();
        GlobalReport {
            restored: 3,
            failed: vec![],
        }
        .write(&mut out);
        assert_eq!(out, r#","global_state_restored":3"#);
        let mut out = String::new();
        GlobalReport {
            restored: 0,
            failed: vec!["index_template/a\"b".into(), "stored_script/s".into()],
        }
        .write(&mut out);
        assert_eq!(
            out,
            r#","global_state_restored":0,"failed_global_state":["index_template/a\"b","stored_script/s"]"#
        );
    }

    #[test]
    fn the_refusal_is_a_400_naming_the_set() {
        let r = global_state_too_large("stored_scripts", "1000 documents");
        assert_eq!((r.status, r.kind), (400, "illegal_argument_exception"));
        assert!(r.reason.contains("[stored_scripts]"), "{}", r.reason);
        assert!(r.reason.contains("1000 documents"), "{}", r.reason);
    }
}
