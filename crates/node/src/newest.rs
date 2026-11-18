//! What a writer answers when a reader asks for the commit it last
//! published (`internal:serverless/commit/newest`).
//!
//! The answering side of the shell's `NewestCommits`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/NewestCommits.java`:
//! `handle`, `local`). The answer is the manifest the shard's last publish
//! produced, held in memory: nothing is read from the store, and a shard
//! this node does not hold, or has not published, answers none.

use std::sync::Arc;

use oss_transport::actions::push::{NewestRequest, NewestResponse};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::writer::WriterRole;

/// Answers `internal:serverless/commit/newest` on a writer.
pub struct NewestHandler {
    role: Arc<WriterRole>,
}

impl NewestHandler {
    /// A handler over `role`.
    pub fn new(role: Arc<WriterRole>) -> Self {
        Self { role }
    }
}

impl Handler for NewestHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let ask = NewestRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed newest-commit ask", "BAD_REQUEST"))?;
        // Keyed by uuid and shard: a uuid names one index, so the ask's name
        // adds nothing the shell's (name, uuid, shard) key does not.
        let commit = u32::try_from(ask.shard)
            .ok()
            .and_then(|shard| self.role.newest_commit(&ask.index_uuid, shard))
            // ALLOC: one manifest copy an answer, at most MAX_NEWEST_BYTES.
            .map(|manifest| manifest.to_vec());
        NewestResponse { commit }
            .write(out)
            .map_err(|_| RemoteException::status("newest-commit answer", "INTERNAL_SERVER_ERROR"))
    }
}
