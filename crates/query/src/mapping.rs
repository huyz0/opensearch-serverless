//! The fields of an index mapping, flattened to dotted paths.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::Error;

/// How deep a mapping's objects may nest before it is refused.
const MAX_DEPTH: usize = 32;
/// How many fields a mapping may flatten to before it is refused.
const MAX_FIELDS: usize = 20_000;

/// One leaf field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The mapping type (`keyword`, `text`, `long`, ...).
    pub es_type: String,
    /// Whether a `keyword` sub-field exists (a `text` field's exact form).
    pub keyword_sub: bool,
}

impl Field {
    /// The type name the plugin reports for the field.
    #[must_use]
    pub fn sql_type(&self) -> &str {
        match self.es_type.as_str() {
            "date" | "date_nanos" => "timestamp",
            "half_float" => "float",
            "scaled_float" => "double",
            other => other,
        }
    }
}

/// The leaf fields of a mapping, by dotted path.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    fields: BTreeMap<String, Field>,
}

impl Mapping {
    /// Reads a mapping's `properties` (the body of `_mapping` for one index, without its name).
    ///
    /// # Errors
    ///
    /// A mapping that nests or grows past its bound.
    pub fn from_json(mapping: &Value) -> Result<Self, Error> {
        let mut m = Self::default();
        if let Some(props) = mapping.get("properties") {
            m.walk(props, "", 0)?;
        }
        Ok(m)
    }

    fn walk(&mut self, props: &Value, prefix: &str, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::unsupported("the mapping nests too deep"));
        }
        let Some(obj) = props.as_object() else {
            return Ok(());
        };
        for (name, def) in obj {
            // ALLOC: one path, bounded by the mapping's depth and name lengths.
            let path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            if let Some(sub) = def.get("properties") {
                self.walk(sub, &path, depth.saturating_add(1))?;
                continue;
            }
            let es_type = def
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("object")
                .to_owned();
            let keyword_sub = def
                .get("fields")
                .and_then(|f| f.get("keyword"))
                .and_then(|k| k.get("type"))
                .and_then(Value::as_str)
                == Some("keyword");
            if self.fields.len() >= MAX_FIELDS {
                return Err(Error::unsupported("the mapping has too many fields"));
            }
            self.fields.insert(
                path,
                Field {
                    es_type,
                    keyword_sub,
                },
            );
        }
        Ok(())
    }

    /// The leaf at `path`.
    #[must_use]
    pub fn field(&self, path: &str) -> Option<&Field> {
        self.fields.get(path)
    }

    /// The top-level names in the plugin's `SELECT *` order, each with its leaf when
    /// it is one (an object is a single column holding the object).
    pub(crate) fn star_columns(&self) -> Vec<(String, Option<&Field>)> {
        let mut names: Vec<String> = self
            .fields
            .keys()
            .map(|k| k.split('.').next().unwrap_or(k).to_owned())
            .collect();
        names.dedup();
        crate::hashorder::iteration_order(names)
            .into_iter()
            .map(|n| {
                let leaf = self.fields.get(&n);
                (n, leaf)
            })
            .collect()
    }

    /// Every column `DESCRIBE` lists, with its mapping type: the leaves and the objects that
    /// hold them, in the order the plugin's hash table gives (a Java `HashMap` filled in name
    /// order, as `SELECT *` is).
    pub(crate) fn describe_columns(&self) -> Vec<(String, String)> {
        let mut all: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        for (path, f) in &self.fields {
            all.insert(path.clone(), f.es_type.clone());
            let mut at = 0;
            while let Some(dot) = path.get(at..).and_then(|r| r.find('.')) {
                let end = at.saturating_add(dot);
                if let Some(parent) = path.get(..end) {
                    all.entry(parent.to_owned())
                        .or_insert_with(|| "object".to_owned());
                }
                at = end.saturating_add(1);
            }
        }
        let order = crate::hashorder::iteration_order(all.keys().cloned().collect());
        order
            .into_iter()
            .map(|n| {
                let ty = all.get(&n).cloned().unwrap_or_default();
                (n, ty)
            })
            .collect()
    }

    /// Every leaf in name order.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &Field)> {
        self.fields.iter().map(|(k, v)| (k.as_str(), v))
    }
}
