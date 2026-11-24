//! A bucket member's commit register.
//!
//! Ported from `BucketCommits.member` (huyz0/OpenSearch
//! `feature/serverlessshell`). A member publishes its commit as its entry in
//! its bucket's manifest. The generation a publisher sees is the entry's
//! version, so a sibling's swap of the same manifest never looks like this
//! member's commit moving.

use std::collections::BTreeMap;
use std::sync::Arc;

use oss_blob::SwapOutcome;
use oss_blob::register::ABSENT_GENERATION;

use super::{BucketCommits, MemberOutcome, Swap};
use crate::publish::{CommitRegister, PublishError, RegisterValue};

/// One member's commit in its bucket's manifest.
#[derive(Debug)]
pub struct MemberRegister {
    commits: Arc<BucketCommits>,
    bucket: String,
    index_uuid: String,
}

impl MemberRegister {
    /// The register of the index `index_uuid`, a member of `bucket`.
    #[must_use]
    pub fn new(commits: Arc<BucketCommits>, bucket: &str, index_uuid: &str) -> Self {
        Self {
            commits,
            // ALLOC: one id a member register.
            bucket: bucket.to_owned(),
            // ALLOC: as above.
            index_uuid: index_uuid.to_owned(),
        }
    }
}

impl CommitRegister for MemberRegister {
    fn read(&self) -> Result<Option<RegisterValue>, PublishError> {
        Ok(self
            .commits
            .member_commit(&self.bucket, &self.index_uuid)?
            .map(|member| RegisterValue {
                generation: member.version,
                bytes: member.commit,
            }))
    }

    fn swap(&self, expected_generation: u64, value: &[u8]) -> Result<SwapOutcome, PublishError> {
        // An absent generation is a member with no commit: version 0.
        debug_assert_eq!(ABSENT_GENERATION, 0);
        // ALLOC: one swap and one commit copy a publish, at most manifest-sized.
        let swaps = BTreeMap::from([(
            self.index_uuid.clone(),
            Swap::new(expected_generation, value.to_vec()),
        )]);
        let outcomes = self.commits.swap_all(&self.bucket, swaps)?;
        Ok(match outcomes.get(&self.index_uuid) {
            Some(MemberOutcome::Applied(version)) => SwapOutcome::Applied(*version),
            Some(MemberOutcome::Conflict(seen)) => SwapOutcome::Conflict(*seen),
            None => SwapOutcome::Conflict(None),
        })
    }
}
