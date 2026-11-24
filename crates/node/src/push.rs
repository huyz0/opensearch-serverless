//! What a reader does when a shard's writer pushes it a commit
//! (`internal:serverless/commit/push`).
//!
//! Ported from the receive side of the shell's `CommitPush`
//! (`serverless/shell/src/main/java/org/opensearch/serverless/transport/CommitPush.java`:
//! `handle`, `take`, `move`). A push for an index this node has open on an
//! older commit takes the index out of the open set and keeps the pushed
//! commit, so the next search opens on it with one GET. Nothing is opened
//! here: an index nobody searches again makes no request, which is what the
//! shell's eager move cannot say. A push for an index not open, or for the
//! commit already open, is dropped: the open that follows reads the register.

use std::sync::Arc;

use oss_blob::BlobStore;
use oss_reader::commit_push::{ShardKey, newer, stage_of};
use oss_transport::actions::push::{PushRequest, PushResponse};
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use crate::search::{SearchService, lock_pushed};

impl<S: BlobStore + 'static> SearchService<S> {
    /// Takes a commit the writer of shard `shard` of `index` (with `uuid`)
    /// pushed: `commit` is its register value. Gives whether the shard is
    /// open here as a reader, which is what the writer asks.
    pub fn take_push(&self, index: &str, uuid: &str, shard: u32, commit: &[u8]) -> bool {
        let key = crate::search::open_shard::set_key(index, shard);
        let open = self.lock().peek(&key).cloned();
        let Some(held) = open else {
            return false;
        };
        if held.shard.meta.uuid != uuid {
            return false;
        }
        let Some(stage) = stage_of(commit) else {
            return true;
        };
        if !newer(&stage, &held.stage) {
            return true;
        }
        // Kept for the open that follows; lost when the mailbox is full, and
        // then that open reads the register.
        let _ = lock_pushed(&self.pushed).push(
            &ShardKey::new(index, shard),
            uuid,
            commit,
            self.now_millis(),
        );
        // A search that holds the old index finishes on it.
        let released = self.lock().release(&key);
        drop(released);
        true
    }

    /// Keeps `commit`, which this node's own writer published, for the next
    /// open of the shard when it is not open now: the register may still name
    /// a writer that has since died, and that open would follow it. Kept as
    /// long as any pushed commit is (ten seconds); a later open reads the
    /// register. Gives whether the shard is open.
    pub fn take_own(&self, index: &str, uuid: &str, shard: u32, commit: &[u8]) -> bool {
        if self.take_push(index, uuid, shard, commit) {
            return true;
        }
        let key = crate::search::open_shard::set_key(index, shard);
        if self.lock().peek(&key).is_none() {
            let _ = lock_pushed(&self.pushed).push(
                &ShardKey::new(index, shard),
                uuid,
                commit,
                self.now_millis(),
            );
        }
        false
    }
}

/// Answers `internal:serverless/commit/push` on a reader.
pub struct PushHandler<S: BlobStore + 'static> {
    service: Arc<SearchService<S>>,
}

impl<S: BlobStore + 'static> PushHandler<S> {
    /// A handler over `service`.
    pub fn new(service: Arc<SearchService<S>>) -> Self {
        Self { service }
    }
}

impl<S: BlobStore + 'static> Handler for PushHandler<S> {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let push = PushRequest::read(&mut Input::new(request.message))
            .map_err(|_| RemoteException::status("malformed commit push", "BAD_REQUEST"))?;
        let shard = u32::try_from(push.shard).unwrap_or(u32::MAX);
        let open = self
            .service
            .take_push(&push.index_name, &push.index_uuid, shard, &push.commit);
        PushResponse { open }
            .write(out)
            .map_err(|_| RemoteException::status("commit push answer", "INTERNAL_SERVER_ERROR"))
    }
}
