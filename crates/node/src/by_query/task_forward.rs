//! A task read or cancel forwarded to the node that runs the task: the
//! owner's handler, and the send side. The node is found in the leases this
//! node already holds, so a forward makes no request to the store.

use std::sync::Arc;
use std::time::Duration;

use oss_transport::actions::tasks::{TASKS_ACTION, TaskRequest, TaskResponse};
use oss_transport::auth::Authenticator;
use oss_transport::client::Client;
use oss_transport::exception::RemoteException;
use oss_transport::server::{Handler, Request};
use oss_transport::stream::{Input, Output};

use super::now_millis;
use super::tasks::Tasks;
use crate::forward::ForwardError;
use crate::forward_call::{owner_of, signed_call};
use crate::push_link::Leases;

/// How long a forwarded task call waits for the node that runs the task.
pub const TASK_TIMEOUT: Duration = Duration::from_secs(10);

/// Answers forwarded task calls from this node's tasks. It never forwards
/// again: a call names a number on this node.
pub struct TaskHandler {
    tasks: Arc<Tasks>,
}

impl TaskHandler {
    /// A handler answering from `tasks`.
    #[must_use]
    pub fn new(tasks: Arc<Tasks>) -> Self {
        Self { tasks }
    }
}

impl Handler for TaskHandler {
    fn handle(&self, request: &Request<'_>, out: &mut Output<'_>) -> Result<(), RemoteException> {
        let call = TaskRequest::read(&mut Input::new(request.message)).map_err(|e| {
            // ALLOC: error path; one message.
            RemoteException::status(&format!("bad task request: {e}"), "BAD_REQUEST")
        })?;
        self.tasks.call(call, now_millis()).write(out).map_err(|_| {
            RemoteException::status("the answer is too large to send", "INTERNAL_SERVER_ERROR")
        })
    }
}

/// The call a task id of another node makes.
pub trait TaskSend: Send + Sync {
    /// Sends `request` to node `node_id` and reads its answer.
    ///
    /// # Errors
    ///
    /// [`ForwardError`]: nothing sent, or the call failed.
    fn call(&self, node_id: &str, request: TaskRequest) -> Result<TaskResponse, ForwardError>;
}

/// Sends task calls over the transport client, signed.
pub struct TaskForwarder<L> {
    client: Arc<Client>,
    auth: Arc<Authenticator>,
    leases: L,
}

impl<L: Leases> TaskForwarder<L> {
    /// A forwarder calling through `client`, signing with `auth`, finding
    /// nodes in `leases`.
    pub fn new(client: Arc<Client>, auth: Arc<Authenticator>, leases: L) -> Self {
        Self {
            client,
            auth,
            leases,
        }
    }
}

impl<L: Leases> TaskSend for TaskForwarder<L> {
    fn call(&self, node_id: &str, request: TaskRequest) -> Result<TaskResponse, ForwardError> {
        let (lease, address) = owner_of(&self.leases, node_id)?;
        let answer = signed_call(
            &self.client,
            &self.auth,
            TASKS_ACTION,
            (&lease, address),
            TASK_TIMEOUT,
            16,
            |out| request.write(out),
        )?;
        TaskResponse::read(&mut Input::new(answer.message())).map_err(|_| ForwardError::BadAnswer)
    }
}
