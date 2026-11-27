//! The `update` items of a bulk, resolved over a [`WriterRole`] by the
//! decision the update route makes.

use oss_writer::bulk::{Action, IndexInfo, Resolved, UpdateFailure, Updates};

use crate::update_body::read;
use crate::update_decide::{UNASSIGNED, check, resolve_once};
use crate::writer::WriterRole;

/// Resolves a bulk's update items through `role`.
pub struct RoleUpdates<'r> {
    role: &'r WriterRole,
}

impl<'r> RoleUpdates<'r> {
    /// Updates read and decided through `role`.
    pub fn new(role: &'r WriterRole) -> Self {
        Self { role }
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
        // A stored script by id is not served in a bulk item: the message says to send the source.
        let body = read(body, None)?;
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
