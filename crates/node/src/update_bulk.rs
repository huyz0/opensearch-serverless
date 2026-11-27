//! The `update` items of a bulk, resolved over a [`WriterRole`] by the
//! decision the update route makes.

use oss_writer::bulk::{Action, IndexInfo, Resolved, UpdateFailure, Updates};

use oss_types::stored_scripts::StoredScripts;

use crate::update_body::read;
use crate::update_decide::{UNASSIGNED, check, resolve_once};
use crate::writer::WriterRole;

/// Resolves a bulk's update items through `role`.
pub struct RoleUpdates<'r> {
    role: &'r WriterRole,
    scripts: Option<&'r dyn StoredScripts>,
}

impl<'r> RoleUpdates<'r> {
    /// Updates read and decided through `role`, a script by `id` resolved
    /// through `scripts`.
    pub fn new(role: &'r WriterRole, scripts: Option<&'r dyn StoredScripts>) -> Self {
        Self { role, scripts }
    }
}

impl Updates for RoleUpdates<'_> {
    fn resolve(
        &mut self,
        index: &IndexInfo,
        shard: u32,
        action: &Action,
        id: &str,
        body: &[u8],
    ) -> Result<Resolved, UpdateFailure> {
        let body = read(body, self.scripts)?;
        check(&body, 0, action.require_alias)?;
        let (if_seq_no, if_primary_term) = (action.expect.if_seq_no, action.expect.if_primary_term);
        debug_assert!(if_seq_no == UNASSIGNED || if_seq_no >= 0);
        resolve_once(
            self.role,
            index,
            shard,
            id,
            &body,
            if_seq_no,
            if_primary_term,
        )
    }
}
