//! The write block an index's settings name, and what it refuses.

use oss_rest::Exception;

use super::Kind;

/// The write block an index's settings name: `index.blocks.write` and
/// `index.blocks.read_only` stop every write, `index.blocks.read_only_allow_delete`
/// stops every write but a delete.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteBlock {
    /// Writes pass.
    #[default]
    None,
    /// Only a delete passes.
    AllowDelete,
    /// No write passes.
    Full,
}

impl WriteBlock {
    /// The refusal for a write of `kind` to `index`, none when it passes.
    pub(super) fn refusal(self, kind: &Kind, index: &str) -> Option<Exception> {
        let block = match self {
            Self::Full => "FORBIDDEN/8/index write (api)",
            Self::AllowDelete if !matches!(kind, Kind::Delete) => {
                "FORBIDDEN/12/disk usage exceeded flood-stage watermark, index has read-only-allow-delete block"
            }
            _ => return None,
        };
        Some(Exception::new(
            "cluster_block_exception",
            // ALLOC: error path; one message.
            format!("index [{index}] blocked by: [{block}];"),
        ))
    }
}
