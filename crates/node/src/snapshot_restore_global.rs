//! The global-state step of a restore (ADR 0068): the checks that precede
//! the first write, and the puts that follow once every restored index is
//! whole. A document or setting that cannot be applied never removes an
//! index: it is named in the answer, or in the node's log for a restore
//! that does not wait. Nothing is deleted: a document is put by name and
//! the settings are merged key by key.

use oss_coordinator::pipeline_admin::{concrete_id, validate, validate_search};
use oss_coordinator::snapshot_restore_admin::{
    GlobalReport, RestoreRequest, global_state_too_large,
};
use oss_coordinator::template_admin::{Kind, concrete_name, validate as validate_template};
use oss_format::{
    GlobalState, MAX_CLUSTER_SETTINGS_BYTES, MAX_DOC_SET_MARKER_BYTES, MAX_DOCS,
    MAX_GLOBAL_STATE_BYTES, Scalar, StateSet,
};
use oss_meta::{ClusterConfig, ConfigValue, DocSet, MetaError};
use oss_rest::RestError;

use crate::route_util::{refuse, refused};
use crate::snapshot_global_capture::kind;
use crate::snapshot_restore_route::{SnapshotRestoreService, unavailable};

/// What the step holds while it runs: the decoded state, a marker buffer
/// and the settings buffer (a written setting takes up to 6 bytes a byte).
pub(crate) const RESERVE_BYTES: usize =
    MAX_GLOBAL_STATE_BYTES + MAX_DOC_SET_MARKER_BYTES + SETTINGS_BUF;

const SETTINGS_BUF: usize = 6 * MAX_CLUSTER_SETTINGS_BYTES;

/// How an applied or failed document is named in the answer.
const fn label(set: StateSet) -> &'static str {
    match set {
        StateSet::IndexTemplates => "index_template",
        StateSet::ComponentTemplates => "component_template",
        StateSet::IngestPipelines => "ingest_pipeline",
        StateSet::SearchPipelines => "search_pipeline",
        StateSet::StoredScripts => "stored_script",
    }
}

/// Whether the PUT route of `set` would accept this document under this name.
fn valid(set: StateSet, name: &str, doc: &[u8]) -> bool {
    match set {
        StateSet::IndexTemplates => {
            concrete_name(name).is_ok() && validate_template(Kind::Index, doc).is_ok()
        }
        StateSet::ComponentTemplates => {
            concrete_name(name).is_ok() && validate_template(Kind::Component, doc).is_ok()
        }
        StateSet::IngestPipelines => concrete_id(name).is_ok() && validate(doc).is_ok(),
        StateSet::SearchPipelines => concrete_id(name).is_ok() && validate_search(doc).is_ok(),
        StateSet::StoredScripts => {
            !name.starts_with('_') && crate::script_route::validate_stored(doc)
        }
    }
}

fn text(value: &Scalar) -> String {
    match value {
        Scalar::Str(s) | Scalar::Number(s) => s.clone(),
        Scalar::Int(n) => n.to_string(),
        Scalar::Bool(b) => b.to_string(),
    }
}

fn failed(e: &MetaError) -> RestError {
    match e {
        MetaError::Blob(_) => unavailable(e),
        // ALLOC: error path; the failure's words.
        other => refuse(500, "restore_failure", other.to_string()),
    }
}

impl SnapshotRestoreService {
    /// The global state to apply: none unless the restore asked for it and
    /// the record holds it.
    pub(crate) fn global_state_of(
        record: &[u8],
        asked: &RestoreRequest,
    ) -> Result<Option<GlobalState>, RestError> {
        if !asked.include_global_state {
            return Ok(None);
        }
        GlobalState::find(record)
            .map(|found| Some(found.unwrap_or_default()))
            .map_err(|e| refuse(500, "snapshot_unreadable", e.to_string()))
    }

    /// Refuses, before anything is written, a restore after which a set
    /// would hold more than 1,000 documents or the settings pass their
    /// bound. Cost: one GET for each set the state has documents for, and
    /// one for the settings.
    pub(crate) fn check_global(&self, state: &GlobalState) -> Result<(), RestError> {
        // ALLOC: one marker buffer, MAX_DOC_SET_MARKER_BYTES, reserved by the caller.
        let mut marker = vec![0u8; MAX_DOC_SET_MARKER_BYTES];
        for set in StateSet::ALL {
            let docs = state.documents(set);
            if docs.is_empty() {
                continue;
            }
            let held = DocSet::new(self.store.clone(), &self.base, kind(set))
                .and_then(|d| d.names(&mut marker))
                .map_err(|e| failed(&e))?;
            let added = docs.iter().filter(|(n, _)| !held.contains(n)).count();
            if held.len().saturating_add(added) > MAX_DOCS {
                return Err(refused(global_state_too_large(
                    set.key(),
                    "1,000 documents",
                )));
            }
        }
        if state.settings.is_empty() {
            return Ok(());
        }
        // ALLOC: the settings buffer, SETTINGS_BUF, reserved by the caller.
        let mut buf = vec![0u8; SETTINGS_BUF];
        let ConfigValue { mut settings, .. } = ClusterConfig::new(self.store.clone(), &self.base)
            .and_then(|c| c.read(&mut buf))
            .map_err(|e| failed(&e))?;
        // ALLOC: one pair a setting, bounded by the state's size.
        let values: Vec<(&str, String)> = state
            .settings
            .iter()
            .map(|(k, v)| (k.as_str(), text(v)))
            .collect();
        let changes: Vec<(&str, Option<&str>)> =
            values.iter().map(|(k, v)| (*k, Some(v.as_str()))).collect();
        let merged = settings.apply(&changes).is_ok() && settings.write(&mut buf).is_ok();
        if merged {
            Ok(())
        } else {
            Err(refused(global_state_too_large(
                "persistent_settings",
                "256 KiB of settings, or a list the register cannot hold",
            )))
        }
    }

    /// Puts each document of `state` by name and merges its settings. A
    /// document that no longer validates, or whose put fails, is named and
    /// the rest still run; once `cancelled` answers true what is left is
    /// counted, logged and not applied.
    pub(crate) fn apply_global(
        &self,
        state: &GlobalState,
        cancelled: &dyn Fn() -> bool,
    ) -> GlobalReport {
        // ALLOC: one marker buffer, MAX_DOC_SET_MARKER_BYTES, reserved by the caller.
        let mut marker = vec![0u8; MAX_DOC_SET_MARKER_BYTES];
        let mut report = GlobalReport {
            restored: 0,
            // ALLOC: one name for each failure, at most the state's entries.
            failed: Vec::new(),
        };
        let mut skipped = 0usize;
        for set in StateSet::ALL {
            let docs = state.documents(set);
            let put = DocSet::new(self.store.clone(), &self.base, kind(set));
            for (name, doc) in docs {
                if cancelled() {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
                let applied = match &put {
                    Ok(d) if valid(set, name, doc) => d
                        .put(name, doc, false, &mut marker)
                        .map_err(|e| e.to_string()),
                    Ok(_) => Err("it no longer validates".to_owned()),
                    Err(e) => Err(e.to_string()),
                };
                match applied {
                    Ok(()) => report.restored = report.restored.saturating_add(1),
                    Err(why) => {
                        eprintln!(
                            "snapshot restore could not apply {}/{name}: {why}",
                            label(set)
                        );
                        // ALLOC: one name a failed document.
                        report.failed.push(format!("{}/{name}", label(set)));
                    }
                }
            }
        }
        self.apply_settings(state, cancelled, &mut report, &mut skipped);
        if skipped > 0 {
            eprintln!(
                "snapshot restore was cancelled with {skipped} global state entries not applied"
            );
            // ALLOC: one entry, once.
            report.failed.push(format!("cancelled/{skipped}"));
        }
        report
    }

    fn apply_settings(
        &self,
        state: &GlobalState,
        cancelled: &dyn Fn() -> bool,
        report: &mut GlobalReport,
        skipped: &mut usize,
    ) {
        if state.settings.is_empty() {
            return;
        }
        if cancelled() {
            *skipped = skipped.saturating_add(state.settings.len());
            return;
        }
        // ALLOC: the settings buffer, SETTINGS_BUF, reserved by the caller.
        let mut buf = vec![0u8; SETTINGS_BUF];
        // ALLOC: one pair a setting, bounded by the state's size.
        let values: Vec<(&str, String)> = state
            .settings
            .iter()
            .map(|(k, v)| (k.as_str(), text(v)))
            .collect();
        let changes: Vec<(&str, Option<&str>)> =
            values.iter().map(|(k, v)| (*k, Some(v.as_str()))).collect();
        let done = ClusterConfig::new(self.store.clone(), &self.base)
            .and_then(|c| c.update(&changes, &mut buf));
        match done {
            Ok(_) => report.restored = report.restored.saturating_add(changes.len()),
            Err(e) => {
                eprintln!("snapshot restore could not merge the persistent settings: {e}");
                report.failed.push("persistent_settings".to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use oss_blob::{BlobStore, Emulator};
    use oss_meta::{DeleteMode, DescriptorStore};

    use crate::index_delete::{DeleteService, NoSnapshots};

    fn service(blobs: &Arc<dyn BlobStore>) -> SnapshotRestoreService {
        let clock: oss_meta::PitClock = Arc::new(|| 1_700_000_000_000);
        let deletes = Arc::new(
            DeleteService::new(
                blobs,
                "",
                DeleteMode::Conditional,
                Arc::clone(&clock),
                Arc::new(NoSnapshots),
                1 << 16,
                8,
            )
            .unwrap(),
        );
        let descriptors =
            DescriptorStore::new(Arc::clone(blobs), "", DeleteMode::Conditional).unwrap();
        SnapshotRestoreService::new(Arc::clone(blobs), "", descriptors, deletes, clock, true, 64)
    }

    // A cancelled step counts what it did not apply, asks the store for
    // nothing and leaves every document and setting unmade.
    #[test]
    fn a_cancelled_global_state_step_names_the_count_and_applies_nothing() {
        let blobs: Arc<dyn BlobStore> = Arc::new(Emulator::new(1 << 24, 1));
        let service = service(&blobs);
        let mut state = GlobalState::default();
        state.settings.push(("a".into(), Scalar::Bool(true)));
        state.push(StateSet::StoredScripts, "s".into(), b"{}".to_vec());
        state.push(StateSet::IndexTemplates, "t".into(), b"{}".to_vec());
        let report = service.apply_global(&state, &|| true);
        assert_eq!(report.restored, 0);
        assert_eq!(report.failed, ["cancelled/3"]);
        let docs = DocSet::new(Arc::clone(&blobs), "", kind(StateSet::StoredScripts)).unwrap();
        assert!(docs.names(&mut vec![0u8; 1 << 16]).unwrap().is_empty());
    }

    #[test]
    fn a_setting_is_written_as_the_text_the_register_holds() {
        assert_eq!(text(&Scalar::Str("x".into())), "x");
        assert_eq!(text(&Scalar::Int(-3)), "-3");
        assert_eq!(text(&Scalar::Number("1.5".into())), "1.5");
        assert_eq!(text(&Scalar::Bool(true)), "true");
    }

    #[test]
    fn each_set_applies_the_validator_of_its_put_route() {
        assert!(!valid(
            StateSet::StoredScripts,
            "_x",
            br#"{"script":{"lang":"painless","source":"1"}}"#
        ));
        assert!(valid(
            StateSet::StoredScripts,
            "x",
            br#"{"script":{"lang":"painless","source":"1"}}"#
        ));
        assert!(!valid(
            StateSet::StoredScripts,
            "x",
            br#"{"script":{"lang":"nope","source":"1"}}"#
        ));
        assert!(!valid(
            StateSet::IngestPipelines,
            "_p",
            br#"{"processors":[]}"#
        ));
        assert!(!valid(StateSet::IngestPipelines, "p", b"{}"));
        assert!(!valid(StateSet::SearchPipelines, "p", b"[]"));
        assert!(!valid(StateSet::IndexTemplates, "_t", b"{}"));
        assert!(!valid(StateSet::ComponentTemplates, "t", b"[]"));
    }
}
