//! The tasks of register and deploy requests: node memory only, bounded, no blob request.
//!
//! A remote model's register and deploy finish inside the request, so a task is a record
//! of that, kept for a client that reads it by id. The 128 most recent are kept (ADR 0072's
//! bound); a task of another node, or from before a restart, is a 404.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::{MlService, bad, fresh_id, now_millis, refuse};

/// The most finished tasks kept.
const KEPT: usize = 128;
/// The tasks search's default size.
const DEFAULT_SIZE: usize = 10;

/// A task: its id and its record.
type Task = (String, Map<String, Value>);

/// The node's recent tasks, oldest first.
#[derive(Default)]
pub(super) struct Tasks(Mutex<VecDeque<Task>>);

impl Tasks {
    fn push(&self, id: &str, task: Map<String, Value>) {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // ALLOC: one record a task, at most KEPT kept.
        kept.push_back((id.to_owned(), task));
        while kept.len() > KEPT {
            kept.pop_front();
        }
    }

    fn get(&self, id: &str) -> Option<Map<String, Value>> {
        let kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        kept.iter().find(|(k, _)| k == id).map(|(_, t)| t.clone())
    }

    fn remove(&self, id: &str) -> bool {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let before = kept.len();
        kept.retain(|(k, _)| k != id);
        kept.len() != before
    }

    fn newest(&self, size: usize) -> (usize, Vec<Task>) {
        let kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        (kept.len(), kept.iter().rev().take(size).cloned().collect())
    }
}

fn missing(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Failed to find task with the provided task id: {id}"),
    )
}

impl MlService {
    /// Records a task that finished inside its request and returns its id.
    pub(super) fn finished_task(
        &self,
        model_id: &str,
        task_type: &str,
        is_async: bool,
    ) -> Result<String, RestError> {
        let id = fresh_id()?;
        let t = now_millis();
        let mut task = Map::new();
        task.insert("model_id".into(), json!(model_id));
        task.insert("task_type".into(), json!(task_type));
        task.insert("function_name".into(), json!("REMOTE"));
        task.insert("state".into(), json!("COMPLETED"));
        task.insert("worker_node".into(), json!([self.node_id]));
        task.insert("create_time".into(), json!(t));
        task.insert("last_update_time".into(), json!(t));
        task.insert("is_async".into(), json!(is_async));
        self.tasks.push(&id, task);
        Ok(id)
    }

    fn task_id(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("task_id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a task id is required"))
    }

    /// `GET /tasks/{id}`.
    pub(crate) fn get_task(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::task_id(req)?;
        let task = self.tasks.get(&id).ok_or_else(|| missing(&id))?;
        resp.send(200, JSON, Value::Object(task).to_string().as_bytes())
    }

    /// `DELETE /tasks/{id}`.
    pub(crate) fn delete_task(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::task_id(req)?;
        if !self.tasks.remove(&id) {
            return Err(missing(&id));
        }
        let out = json!({
            "_index": ".plugins-ml-task", "_id": id, "_version": 1, "result": "deleted",
            "forced_refresh": true,
            "_shards": {"total": 1, "successful": 1, "failed": 0},
            "_seq_no": 0, "_primary_term": 1,
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// `GET|POST /tasks/_search`: this node's recent tasks, newest first; `size` bounds the page.
    pub(crate) fn search_tasks(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let body = Self::body_or_empty(req)?;
        let size = body
            .get("size")
            .map_or(Some(DEFAULT_SIZE), |v| {
                v.as_u64().and_then(|n| usize::try_from(n).ok())
            })
            .filter(|n| *n <= KEPT)
            .ok_or_else(|| bad(format!("[size] must be a number up to {KEPT}")))?;
        let (total, newest) = self.tasks.newest(size);
        let hits: Vec<Value> = newest
            .into_iter()
            .map(|(id, task)| {
                json!({"_index": ".plugins-ml-task", "_id": id, "_version": 1, "_seq_no": 0,
                       "_primary_term": 1, "_score": 1.0, "_source": Value::Object(task)})
            })
            .collect();
        let out = json!({
            "took": 1, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"total": {"value": total, "relation": "eq"},
                     "max_score": if hits.is_empty() { Value::Null } else { json!(1.0) },
                     "hits": hits},
        });
        resp.send(200, JSON, out.to_string().as_bytes())
    }
}
