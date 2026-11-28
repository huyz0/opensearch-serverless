//! Remote models: one document each of the `ml/models/` set, naming the connector they call
//! through. Register and deploy finish inside the request and leave a task record.

use oss_format::DocKind;
use oss_rest::{JSON, Request, Response, RestError};
use serde_json::{Map, Value, json};

use super::{MlService, bad, failed, fresh_id, now_millis, refuse, refuse_access_parameters};

/// What every model shows as its `_index`.
const INDEX: &str = ".plugins-ml-model";
/// The algorithms stock knows by name; only `remote` is served here so far.
const KNOWN: [&str; 14] = [
    "remote",
    "text_embedding",
    "sparse_encoding",
    "sparse_tokenize",
    "text_similarity",
    "question_answering",
    "tools",
    "kmeans",
    "rcf",
    "linear_regression",
    "logistic_regression",
    "ad_libsvm",
    "metrics_correlation",
    "localsample_calculator",
];
/// The fields of a model an update may set.
const UPDATABLE: [&str; 3] = ["name", "description", "connector_id"];

pub(super) fn missing(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Failed to find model with the provided model id: {id}"),
    )
}

pub(super) fn connector_missing(id: &str) -> RestError {
    refuse(
        404,
        "status_exception",
        // ALLOC: error path; one message.
        format!("Failed to find connector with the provided connector id: {id}"),
    )
}

fn index_result(id: &str, result: &str) -> Value {
    json!({
        "_index": INDEX, "_id": id, "_version": 1, "result": result,
        "forced_refresh": true,
        "_shards": {"total": 1, "successful": 1, "failed": 0},
        "_seq_no": 0, "_primary_term": 1,
    })
}

fn state_of(model: &Map<String, Value>) -> &str {
    model
        .get("model_state")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

impl MlService {
    pub(super) fn model_id(req: &Request<'_>) -> Result<String, RestError> {
        req.path_param("model_id")
            .map(std::borrow::Cow::into_owned)
            .ok_or_else(|| bad("a model id is required"))
    }

    fn stored_model(
        &self,
        id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Map<String, Value>, RestError> {
        self.stored_in(DocKind::MlModels, id, marker, doc)?
            .ok_or_else(|| missing(id))
    }

    fn write_model(
        &self,
        id: &str,
        model: &Map<String, Value>,
        create: bool,
        marker: &mut [u8],
    ) -> Result<(), RestError> {
        // ALLOC: the document's text, at most the kind's bound, checked by the put.
        let bytes = Value::Object(model.clone()).to_string().into_bytes();
        self.set_of(DocKind::MlModels)?
            .put_counted(id, &bytes, create, marker)
            .map(|_| ())
            .map_err(|e| failed(&e))
    }

    /// Takes the next version of group `group_id` and records it in the group, or fails as
    /// stock does for a group that is not there.
    fn next_group_version(
        &self,
        group_id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<u64, RestError> {
        let Some(mut group) = self.stored(group_id, marker, doc)? else {
            return Err(refuse(
                500,
                "m_l_resource_not_found_exception",
                // ALLOC: error path; one message.
                format!("Model Group Response is empty for {group_id}"),
            ));
        };
        let version = group
            .get("latest_version")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        group.insert("latest_version".into(), json!(version));
        group.insert("last_updated_time".into(), json!(now_millis()));
        self.write(group_id, &group, false, marker)?;
        Ok(version)
    }

    /// `POST /models/_register`: a remote model, naming a connector or carrying one.
    pub(crate) fn register_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let mut body = Self::body(req)?;
        refuse_access_parameters(&body)?;
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| bad("model name is null"))?
            .to_owned();
        let function = body
            .get("function_name")
            .and_then(Value::as_str)
            .map(str::to_lowercase)
            .filter(|f| KNOWN.contains(&f.as_str()))
            .ok_or_else(|| bad("Wrong function name"))?;
        if function != "remote" {
            return Err(refuse(
                501,
                "not_implemented",
                // ALLOC: error path; one message.
                format!("models with function [{function}] are not served by this node yet"),
            ));
        }
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let named = body
            .get("connector_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let connector_id = match (named, body.remove("connector")) {
            (Some(id), _) => {
                if self
                    .stored_in(DocKind::MlConnectors, &id, &mut marker, &mut doc)?
                    .is_none()
                {
                    return Err(connector_missing(&id));
                }
                id
            }
            (None, Some(Value::Object(def))) => self.insert_connector(def, &mut marker)?,
            _ => {
                return Err(bad(
                    "You must provide connector content when creating a remote model without connector id!",
                ));
            }
        };
        let group_id = body
            .get("model_group_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let version = match &group_id {
            Some(g) => self.next_group_version(g, &mut marker, &mut doc)?,
            None => 1,
        };
        let id = fresh_id()?;
        let t = now_millis();
        let mut model = Map::new();
        model.insert("name".into(), json!(name));
        model.insert("model_id".into(), json!(id));
        model.insert("algorithm".into(), json!("REMOTE"));
        if let Some(g) = group_id {
            model.insert("model_group_id".into(), json!(g));
        }
        model.insert("model_version".into(), json!(version.to_string()));
        if let Some(d) = body.get("description").and_then(Value::as_str) {
            model.insert("description".into(), json!(d));
        }
        model.insert("model_state".into(), json!("REGISTERED"));
        model.insert("created_time".into(), json!(t));
        model.insert("last_updated_time".into(), json!(t));
        model.insert("is_hidden".into(), json!(false));
        model.insert("connector_id".into(), json!(connector_id));
        self.write_model(&id, &model, true, &mut marker)?;
        let task = self.finished_task(&id, "REGISTER_MODEL", false)?;
        let out = json!({"task_id": task, "status": "CREATED", "model_id": id});
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// `GET /models/{id}`.
    pub(crate) fn get_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut model = self.stored_model(&id, &mut marker, &mut doc)?;
        model.remove("model_id");
        resp.send(200, JSON, Value::Object(model).to_string().as_bytes())
    }

    /// `PUT /models/{id}`: name, description and connector.
    pub(crate) fn update_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let body = Self::body(req)?;
        refuse_access_parameters(&body)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut model = self.stored_model(&id, &mut marker, &mut doc)?;
        for (key, value) in body {
            if !UPDATABLE.contains(&key.as_str()) || value.is_null() {
                continue;
            }
            let text = value
                .as_str()
                .filter(|t| key != "name" || !t.trim().is_empty())
                .ok_or_else(|| bad(format!("[{key}] must be a non-empty string")))?;
            if key == "connector_id"
                && self
                    .stored_in(DocKind::MlConnectors, text, &mut marker, &mut doc)?
                    .is_none()
            {
                return Err(connector_missing(text));
            }
            model.insert(key, json!(text));
        }
        model.insert("last_updated_time".into(), json!(now_millis()));
        self.write_model(&id, &model, false, &mut marker)?;
        self.cache.forget(&id);
        resp.send(
            200,
            JSON,
            index_result(&id, "updated").to_string().as_bytes(),
        )
    }

    /// `DELETE /models/{id}`: refused while the model is deployed.
    pub(crate) fn delete_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let Some(model) = self.stored_in(DocKind::MlModels, &id, &mut marker, &mut doc)? else {
            return Err(refuse(404, "status_exception", "Failed to find model"));
        };
        if matches!(
            state_of(&model),
            "DEPLOYED" | "DEPLOYING" | "PARTIALLY_DEPLOYED"
        ) {
            return Err(refuse(
                400,
                "status_exception",
                "Model cannot be deleted in deploying or deployed state. \
                 Try undeploy model first then delete",
            ));
        }
        if !self
            .set_of(DocKind::MlModels)?
            .delete(&id, &mut marker)
            .map_err(|e| failed(&e))?
        {
            return Err(refuse(404, "status_exception", "Failed to find model"));
        }
        self.cache.forget(&id);
        resp.send(
            200,
            JSON,
            index_result(&id, "deleted").to_string().as_bytes(),
        )
    }

    /// `POST /models/{id}/_deploy`: a remote model has nothing to load, so it is deployed
    /// when the request returns.
    pub(crate) fn deploy_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut model = self.stored_model(&id, &mut marker, &mut doc)?;
        if state_of(&model) != "DEPLOYED" {
            let t = now_millis();
            model.insert("model_state".into(), json!("DEPLOYED"));
            model.insert("last_deployed_time".into(), json!(t));
            model.insert("last_updated_time".into(), json!(t));
            model.insert("auto_redeploy_retry_times".into(), json!(0));
            model.insert("planning_worker_node_count".into(), json!(1));
            model.insert("current_worker_node_count".into(), json!(1));
            model.insert("planning_worker_nodes".into(), json!([self.node_id]));
            model.insert("deploy_to_all_nodes".into(), json!(true));
            self.write_model(&id, &model, false, &mut marker)?;
        }
        let task = self.finished_task(&id, "DEPLOY_MODEL", true)?;
        let out = json!({"task_id": task, "task_type": "DEPLOY_MODEL", "status": "COMPLETED"});
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// `POST /models/{id}/_undeploy`: per node, `undeployed` or `not_found`.
    pub(crate) fn undeploy_model(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        let id = Self::model_id(req)?;
        let mut marker = self.marker(req)?;
        let mut doc = Self::doc_buffer(req)?;
        let mut model = self.stored_model(&id, &mut marker, &mut doc)?;
        let word = if state_of(&model) == "DEPLOYED" {
            for key in [
                "last_deployed_time",
                "auto_redeploy_retry_times",
                "planning_worker_node_count",
                "current_worker_node_count",
                "planning_worker_nodes",
                "deploy_to_all_nodes",
            ] {
                model.remove(key);
            }
            model.insert("model_state".into(), json!("UNDEPLOYED"));
            model.insert("last_updated_time".into(), json!(now_millis()));
            self.write_model(&id, &model, false, &mut marker)?;
            "undeployed"
        } else {
            "not_found"
        };
        let out = json!({ self.node_id.clone(): {"stats": {id: word}} });
        resp.send(200, JSON, out.to_string().as_bytes())
    }

    /// The ids of the models that call through connector `connector_id`: a read of every
    /// model, so refused past `TOKEN_SCAN_MAX` models.
    pub(super) fn models_using(
        &self,
        connector_id: &str,
        marker: &mut [u8],
        doc: &mut [u8],
    ) -> Result<Vec<String>, RestError> {
        let mut ids = self
            .set_of(DocKind::MlModels)?
            .names(marker)
            .map_err(|e| failed(&e))?;
        if ids.len() > super::search::TOKEN_SCAN_MAX {
            return Err(bad(format!(
                "the set holds {} models, too many to check which use this connector; \
                 delete models you no longer use first",
                ids.len()
            )));
        }
        ids.sort();
        let mut using = Vec::new();
        for id in ids {
            let model = self.stored_in(DocKind::MlModels, &id, marker, doc)?;
            if model
                .and_then(|m| {
                    m.get("connector_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .is_some_and(|c| c == connector_id)
            {
                using.push(id);
            }
        }
        Ok(using)
    }

    /// `GET|POST /models/_search`.
    pub(crate) fn search_models(
        &self,
        req: &mut Request<'_>,
        resp: &mut Response<'_>,
    ) -> Result<(), RestError> {
        self.search_set(DocKind::MlModels, INDEX, req, resp)
    }
}
