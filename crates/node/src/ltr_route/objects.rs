//! The objects of a store as the plugin's API shapes them: a feature set
//! and a model, read from a request, checked, and written back.

use oss_reader::ltr::{Evaluator, MAX_FEATURES, ModelError};
use serde_json::{Map, Value, json};

/// A feature set: its name and the features, in order.
pub(super) struct FeatureSet {
    pub(super) name: String,
    pub(super) features: Vec<Value>,
}

/// A refusal's reason, the `illegal_argument_exception` of a 400.
pub(super) type Reason = String;

fn bad<T>(reason: impl Into<String>) -> Result<T, Reason> {
    Err(reason.into())
}

/// A name the plugin accepts: not empty, not `_`-prefixed.
fn check_name(what: &str, name: &str) -> Result<(), Reason> {
    if name.is_empty() {
        return bad(format!("{what} name must not be empty"));
    }
    if name.starts_with('_') {
        return bad(format!("{what} name [{name}] must not start with '_'"));
    }
    Ok(())
}

/// The features of an array, each as `{name, params, template_language,
/// template}`, names unique.
pub(super) fn features(value: &Value) -> Result<Vec<Value>, Reason> {
    let Some(array) = value.as_array() else {
        return bad("[features] must be an array");
    };
    if array.len() > MAX_FEATURES {
        return bad(format!(
            "a feature set holds at most {MAX_FEATURES} features"
        ));
    }
    // ALLOC: one entry per feature, at most MAX_FEATURES, bounded by the body.
    let mut out: Vec<Value> = Vec::new();
    for feature in array {
        let Some(object) = feature.as_object() else {
            return bad("a feature must be an object");
        };
        let Some(name) = object.get("name").and_then(Value::as_str) else {
            return bad("a feature needs a [name]");
        };
        check_name("feature", name)?;
        if out.iter().any(|f| f["name"] == name) {
            return bad(format!("feature [{name}] is defined twice"));
        }
        let template = match object.get("template") {
            Some(t @ (Value::Object(_) | Value::String(_))) => t.clone(),
            _ => {
                return bad(format!(
                    "feature [{name}] needs a [template], an object or a string"
                ));
            }
        };
        let params = match object.get("params") {
            None | Some(Value::Null) => json!([]),
            Some(p) if p.as_array().is_some_and(|a| a.iter().all(Value::is_string)) => p.clone(),
            Some(_) => {
                return bad(format!(
                    "feature [{name}] [params] must be an array of strings"
                ));
            }
        };
        match object.get("template_language").and_then(Value::as_str) {
            None | Some("mustache") => {}
            Some(other) => {
                return bad(format!(
                    "feature [{name}] template_language [{other}] is not supported; use mustache"
                ));
            }
        }
        out.push(json!({
            "name": name,
            "params": params,
            "template_language": "mustache",
            "template": template,
        }));
    }
    Ok(out)
}

impl FeatureSet {
    /// Reads a put's body, `{"featureset":{"name"?,"features":[..]}}`; the
    /// name is the path's.
    pub(super) fn from_put(name: &str, body: &Value) -> Result<Self, Reason> {
        check_name("feature set", name)?;
        let Some(set) = body.get("featureset").and_then(Value::as_object) else {
            return bad("the body must hold a [featureset] object");
        };
        if let Some(inner) = set.get("name").and_then(Value::as_str)
            && inner != name
        {
            return bad(format!(
                "the feature set [{inner}] does not match the path name [{name}]"
            ));
        }
        let features = features(set.get("features").unwrap_or(&Value::Null))?;
        // ALLOC: one name, once.
        Ok(Self {
            name: name.to_owned(),
            features,
        })
    }

    /// Reads the `{"name","features"}` object a model put embeds.
    pub(super) fn from_embedded(set: &Value) -> Result<Self, Reason> {
        let Some(name) = set.get("name").and_then(Value::as_str) else {
            return bad("the [feature_set] needs a [name]");
        };
        check_name("feature set", name)?;
        let features = features(set.get("features").unwrap_or(&Value::Null))?;
        // ALLOC: one name, once.
        Ok(Self {
            name: name.to_owned(),
            features,
        })
    }

    /// Reads a stored document.
    pub(super) fn from_stored(doc: &Value) -> Result<Self, Reason> {
        let set = doc.get("featureset").and_then(Value::as_object);
        let (Some(set), Some(name)) = (set, doc.get("name").and_then(Value::as_str)) else {
            return bad("a stored feature set is unreadable");
        };
        Ok(Self {
            // ALLOC: one name, once.
            name: name.to_owned(),
            features: set
                .get("features")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// The names of the features, in order.
    pub(super) fn names(&self) -> Vec<String> {
        // ALLOC: one name per feature, at most MAX_FEATURES.
        self.features
            .iter()
            .filter_map(|f| f["name"].as_str().map(str::to_owned))
            .collect()
    }

    /// The `{"featureset":{..}}` object as the plugin's API nests it.
    pub(super) fn body(&self) -> Value {
        json!({"name": self.name, "features": self.features})
    }

    /// The stored document.
    pub(super) fn stored(&self) -> Value {
        json!({"name": self.name, "type": "featureset", "featureset": self.body()})
    }
}

/// A model read from a request, checked against its feature set.
pub(super) struct Model {
    pub(super) name: String,
    kind: String,
    definition: String,
}

impl Model {
    /// Reads `{"model":{"name"?,"model":{"type","definition"}}}`; the name
    /// is the path's when given, else the body's.
    pub(super) fn from_body(
        path_name: Option<&str>,
        body: &Value,
    ) -> Result<(Self, Option<Value>), Reason> {
        let Some(outer) = body.get("model").and_then(Value::as_object) else {
            return bad("the body must hold a [model] object");
        };
        let body_name = outer.get("name").and_then(Value::as_str);
        let name = match (path_name, body_name) {
            (Some(p), Some(b)) if p != b => {
                return bad(format!(
                    "the model [{b}] does not match the path name [{p}]"
                ));
            }
            (Some(n), _) | (None, Some(n)) => n,
            (None, None) => return bad("a model needs a [name]"),
        };
        check_name("model", name)?;
        let Some(inner) = outer.get("model").and_then(Value::as_object) else {
            return bad("a model needs a [model] with a [type] and a [definition]");
        };
        let Some(kind) = inner.get("type").and_then(Value::as_str) else {
            return bad("a model needs a [type]");
        };
        let definition = match inner.get("definition") {
            Some(Value::String(s)) => s.clone(),
            Some(v @ Value::Object(_)) => v.to_string(),
            _ => return bad("a model needs a [definition], a string or an object"),
        };
        // ALLOC: two strings, once, bounded by the body.
        Ok((
            Self {
                name: name.to_owned(),
                kind: kind.to_owned(),
                definition,
            },
            outer.get("feature_set").cloned(),
        ))
    }

    /// Checks that the definition parses against the set's features.
    pub(super) fn check(&self, set: &FeatureSet) -> Result<(), Reason> {
        Evaluator::parse(&self.kind, &self.definition, &set.names())
            .map(|_| ())
            .map_err(|e: ModelError| e.reason().to_owned())
    }

    /// The stored document, the set embedded as the plugin keeps it.
    pub(super) fn stored(&self, set: &FeatureSet) -> Value {
        let mut inner = Map::new();
        inner.insert("type".into(), json!(self.kind));
        inner.insert("definition".into(), json!(self.definition));
        json!({
            "name": self.name,
            "type": "model",
            "model": {"name": self.name, "model": inner, "feature_set": set.body()},
        })
    }
}
