//! The alias step of a restore (ADR 0065): the checks that precede the first
//! write, and the one mutation for each distinct alias that follows once
//! every restored index is unmarked. An alias that cannot be added never
//! removes an index: it is named in the answer, or in the node's log for a
//! restore that does not wait.

use std::time::Duration;

use oss_coordinator::snapshot_restore_admin::Target as Restored;
use oss_coordinator::snapshot_restore_admin::{AliasPlan, AliasReport};
use oss_format::RestoreIndex;
use oss_meta::{AliasClock, AliasMutations, Target, validate_index_or_alias};
use oss_rest::RestError;

use crate::route_util::refuse;
use crate::snapshot_restore_route::{SnapshotRestoreService, unavailable};

/// The bytes of one alias register buffer, and how long a mutation's
/// reservation lasts, until the service is told otherwise.
pub(crate) const DEFAULT_BUFFER_BYTES: usize = 1 << 16;
pub(crate) const DEFAULT_LEASE_MILLIS: i64 = 30_000;

/// The refusal for an alias name that cannot be one.
fn invalid(name: &str, why: &str) -> RestError {
    refuse(
        400,
        "invalid_alias_name_exception",
        // ALLOC: error path; one message.
        format!("Invalid alias name [{name}]: {why}"),
    )
}

/// The restored indices whose record was cut at the alias bound.
pub(crate) fn truncated(targets: &[Restored], sources: &[RestoreIndex]) -> Vec<String> {
    targets
        .iter()
        .zip(sources)
        .filter(|(_, s)| s.aliases_truncated)
        // ALLOC: one name an index, at most the take's bound.
        .map(|(t, _)| t.target.clone())
        .collect()
}

impl SnapshotRestoreService {
    /// What the alias step holds while it runs: three register buffers.
    pub(crate) fn alias_bytes(&self) -> usize {
        self.alias_buffer_bytes.saturating_mul(3)
    }

    /// Refuses, before anything is written, an alias name that is not a name
    /// or that an index or data stream holds. Cost: one GET for each
    /// distinct alias.
    pub(crate) fn check_aliases(&self, plan: &[AliasPlan]) -> Result<(), RestError> {
        // ALLOC: one buffer a request, an alias register's size, reserved by the caller.
        let mut buf = vec![0u8; self.alias_buffer_bytes];
        for alias in plan {
            validate_index_or_alias(&alias.name)
                .map_err(|e| invalid(&alias.name, &e.to_string()))?;
            match self.descriptors.resolve(&alias.name, &mut buf) {
                Ok(r) => match r.target {
                    Target::Index(_) => {
                        return Err(invalid(
                            &alias.name,
                            "an index or alias with the same name already exists",
                        ));
                    }
                    Target::Alias(a) if a.data_stream => {
                        return Err(invalid(&alias.name, "it is a data stream"));
                    }
                    Target::Alias(_) | Target::Absent => {}
                },
                Err(e) => return Err(unavailable(&e)),
            }
        }
        Ok(())
    }

    /// Adds each alias of `plan`, in order; one that fails is named and the
    /// rest still run. Once `cancelled` answers true the aliases not yet
    /// added are named as failed without a request.
    pub(crate) fn add_aliases(
        &self,
        plan: &[AliasPlan],
        truncated: Vec<String>,
        cancelled: &dyn Fn() -> bool,
    ) -> AliasReport {
        let now = || i64::try_from((self.clock)()).unwrap_or(i64::MAX);
        let pause = |d: Duration| std::thread::sleep(d);
        let mut mutations = AliasMutations::new(
            &self.descriptors,
            AliasClock {
                now_millis: &now,
                pause: &pause,
            },
            self.alias_lease_millis,
            self.alias_buffer_bytes,
        );
        let mut report = AliasReport {
            restored: 0,
            // ALLOC: one name a failed alias, at most the plan's.
            failed: Vec::new(),
            truncated,
        };
        for alias in plan {
            if cancelled() {
                report.failed.push(alias.name.clone());
                continue;
            }
            match mutations.add_restored(&alias.name, &alias.members) {
                Ok(()) => report.restored = report.restored.saturating_add(1),
                Err(e) => {
                    eprintln!("snapshot restore could not add alias [{}]: {e}", alias.name);
                    report.failed.push(alias.name.clone());
                }
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use oss_blob::{BlobStore, Emulator};
    use oss_meta::{DeleteMode, DescriptorStore};

    use crate::index_delete::{DeleteService, NoSnapshots};

    // A restore that is cancelled names the aliases it has not added, asks
    // the store for nothing and leaves the alias unmade.
    #[test]
    fn a_cancelled_alias_step_names_every_alias_and_makes_none() {
        let blobs: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
        let descriptors =
            || DescriptorStore::new(Arc::clone(&blobs), "", DeleteMode::Conditional).unwrap();
        let deletes = Arc::new(
            DeleteService::new(
                &blobs,
                "",
                DeleteMode::Conditional,
                Arc::clone(&clock),
                Arc::new(NoSnapshots),
                1 << 16,
                8,
            )
            .unwrap(),
        );
        let service = SnapshotRestoreService::new(
            Arc::clone(&blobs),
            "",
            descriptors(),
            deletes,
            clock,
            true,
            64,
        );
        let plan = [AliasPlan {
            name: "al".into(),
            members: vec![("copy".into(), true)],
        }];
        let report = service.add_aliases(&plan, vec!["copy".into()], &|| true);
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed, ["al"]);
        assert_eq!(report.truncated, ["copy"]);
        let mut buf = vec![0u8; 1 << 16];
        assert!(matches!(
            descriptors().resolve("al", &mut buf).unwrap().target,
            Target::Absent
        ));
    }
}
