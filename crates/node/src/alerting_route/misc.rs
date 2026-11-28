//! The routes whose state the node does not hold yet: the legacy
//! destinations (the plugin migrated them to notification channels, so a
//! store answers none), the email account and group searches, the findings
//! search (a document-level monitor writes findings) and the plugin's stats.

use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Value, json};

use super::AlertingService;

/// The cluster name the plugin's stats report.
const CLUSTER_NAME: &str = "opensearch";

fn send(resp: &mut Response<'_>, body: &Value) -> Result<(), RestError> {
    resp.send(200, JSON, body.to_string().as_bytes())
}

impl AlertingService {
    /// `GET /destinations[/{id}]`: the plugin keeps none, as since 2.0 a
    /// destination is a notification channel.
    pub(crate) fn destinations(
        &self,
        _req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        send(resp, &json!({"totalDestinations": 0, "destinations": []}))
    }

    /// `POST|GET /destinations/email_accounts/_search` and
    /// `/email_groups/_search`: no email account or group is stored, because
    /// an email action is refused (spec).
    pub(crate) fn email_search(
        &self,
        _req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        send(
            resp,
            &json!({
                "took": 0,
                "timed_out": false,
                "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
                "hits": {"total": {"value": 0, "relation": "eq"}, "max_score": null, "hits": []},
            }),
        )
    }

    /// `GET /findings/_search`: no document-level monitor runs yet, so none
    /// has written a finding.
    pub(crate) fn findings(
        &self,
        _req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        send(resp, &json!({"total_findings": 0, "findings": []}))
    }

    /// `GET /stats`: this node, scheduling nothing it has not claimed.
    pub(crate) fn stats(
        &self,
        _req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        send(
            resp,
            &json!({
                "_nodes": {"total": 1, "successful": 1, "failed": 0},
                "cluster_name": CLUSTER_NAME,
                "opendistro.scheduled_jobs.enabled": true,
                "plugins.scheduled_jobs.enabled": true,
                "scheduled_job_index_exists": true,
                "scheduled_job_index_status": "green",
                "nodes_on_schedule": 1,
                "nodes_not_on_schedule": 0,
                "nodes": {
                    "local": {
                        "name": "local",
                        "schedule_status": "green",
                        "roles": ["DATA", "INGEST"],
                        "job_scheduling_metrics": {
                            "last_full_sweep_time_millis": 0,
                            "full_sweep_on_time": true,
                        },
                        "jobs_info": {},
                    },
                },
            }),
        )
    }
}
