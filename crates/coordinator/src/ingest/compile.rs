use oss_doc::mapping::json::Json;

use super::{COMMON, MAX_DEPTH, PROCESSORS, Processor, Step, TextOp, Type};

fn text<'a>(config: &'a Json, key: &str) -> Option<&'a str> {
    match config.get(key) {
        Some(Json::Str(s)) => Some(s),
        _ => None,
    }
}

fn flag(config: &Json, key: &str, default: bool) -> Result<bool, String> {
    match config.get(key) {
        None => Ok(default),
        Some(Json::Bool(b)) => Ok(*b),
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        Some(_) => Err(format!("[{key}] must be a boolean")),
    }
}

fn required<'a>(kind: &str, config: &'a Json, key: &str) -> Result<&'a str, String> {
    text(config, key)
        .filter(|s| !s.is_empty())
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        .ok_or_else(|| format!("[{key}] required property is missing for processor [{kind}]"))
}

fn allow(kind: &str, config: &Json, own: &[&str]) -> Result<(), String> {
    let Json::Obj(members) = config else {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        return Err(format!("processor [{kind}] must be an object"));
    };
    match members
        .iter()
        .find(|(k, _)| !COMMON.contains(&k.as_str()) && !own.contains(&k.as_str()))
    {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        Some((k, _)) => Err(format!(
            "processor [{kind}] does not support the option [{k}]"
        )),
        None => Ok(()),
    }
}

fn plain(kind: &str, value: &Json) -> Result<(), String> {
    match value {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        Json::Str(s) if s.contains("{{") => Err(format!(
            "processor [{kind}] does not support templates in values: [{s}]"
        )),
        Json::Arr(items) => items.iter().try_for_each(|i| plain(kind, i)),
        Json::Obj(members) => members.iter().try_for_each(|(_, v)| plain(kind, v)),
        _ => Ok(()),
    }
}

pub(super) fn list(value: &Json, depth: usize) -> Result<Vec<Processor>, String> {
    let Json::Arr(items) = value else {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        return Err("[processors] must be an array of processors".to_owned());
    };
    // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
    items.iter().map(|i| processor(i, depth)).collect()
}

fn processor(item: &Json, depth: usize) -> Result<Processor, String> {
    let Json::Obj(members) = item else {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        return Err("a processor must be an object of one type".to_owned());
    };
    let [(kind, config)] = members.as_slice() else {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        return Err("a processor must be an object of one type".to_owned());
    };
    let kind = kind.as_str();
    let step = match kind {
        "set" => {
            allow(
                kind,
                config,
                &["field", "value", "override", "ignore_empty_value"],
            )?;
            let value = config
                .get("value")
                .ok_or("[value] required property is missing for processor [set]")?;
            plain(kind, value)?;
            Step::Set {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                field: required(kind, config, "field")?.to_owned(),
                value: value.clone(),
                overwrite: flag(config, "override", true)?,
                ignore_empty: flag(config, "ignore_empty_value", false)?,
            }
        }
        "remove" => {
            allow(kind, config, &["field", "ignore_missing"])?;
            let fields = match config.get("field") {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                Some(Json::Str(s)) if !s.is_empty() => vec![s.clone()],
                Some(Json::Arr(items)) if !items.is_empty() => items
                    .iter()
                    .map(|i| match i {
                        Json::Str(s) if !s.is_empty() => Ok(s.clone()),
                        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                        _ => Err("[field] must hold field names".to_owned()),
                    })
                    .collect::<Result<_, _>>()?,
                _ => {
                    return Err(
                        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                        "[field] required property is missing for processor [remove]".to_owned(),
                    );
                }
            };
            Step::Remove {
                fields,
                ignore_missing: flag(config, "ignore_missing", false)?,
            }
        }
        "rename" => {
            allow(kind, config, &["field", "target_field", "ignore_missing"])?;
            Step::Rename {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                field: required(kind, config, "field")?.to_owned(),
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                target: required(kind, config, "target_field")?.to_owned(),
                ignore_missing: flag(config, "ignore_missing", false)?,
            }
        }
        "lowercase" | "uppercase" | "trim" => {
            allow(kind, config, &["field", "target_field", "ignore_missing"])?;
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            let field = required(kind, config, "field")?.to_owned();
            Step::Text {
                target: text(config, "target_field").map_or_else(|| field.clone(), str::to_owned),
                field,
                ignore_missing: flag(config, "ignore_missing", false)?,
                op: match kind {
                    "lowercase" => TextOp::Lower,
                    "uppercase" => TextOp::Upper,
                    _ => TextOp::Trim,
                },
            }
        }
        "append" => {
            allow(kind, config, &["field", "value", "allow_duplicates"])?;
            let values = match config.get("value") {
                Some(Json::Arr(items)) => items.clone(),
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                Some(one) => vec![one.clone()],
                None => {
                    return Err(
                        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                        "[value] required property is missing for processor [append]".to_owned(),
                    );
                }
            };
            values.iter().try_for_each(|v| plain(kind, v))?;
            Step::Append {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                field: required(kind, config, "field")?.to_owned(),
                values,
                allow_duplicates: flag(config, "allow_duplicates", true)?,
            }
        }
        "convert" => {
            allow(
                kind,
                config,
                &["field", "target_field", "ignore_missing", "type"],
            )?;
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            let field = required(kind, config, "field")?.to_owned();
            let to = match required(kind, config, "type")? {
                "integer" => Type::Integer,
                "long" => Type::Long,
                "float" => Type::Float,
                "double" => Type::Double,
                "boolean" => Type::Boolean,
                "string" => Type::Text,
                other => {
                    // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                    return Err(format!(
                        "type [{other}] not supported, cannot convert field."
                    ));
                }
            };
            Step::Convert {
                target: text(config, "target_field").map_or_else(|| field.clone(), str::to_owned),
                field,
                ignore_missing: flag(config, "ignore_missing", false)?,
                to,
            }
        }
        "script" if oss_reader::painless_switch::native_compile() => {
            allow(kind, config, &["source", "lang", "params"])?;
            if !matches!(text(config, "lang"), None | Some("painless")) {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                return Err("processor [script] runs only lang [painless]".to_owned());
            }
            let params = match config.get("params") {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                None => Json::Obj(Vec::new()),
                Some(p @ Json::Obj(_)) => p.clone(),
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                Some(_) => return Err("[params] must be an object".to_owned()),
            };
            Step::Script(super::script::Script {
                // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
                source: required(kind, config, "source")?.to_owned(),
                params: super::script::to_constant(&params),
            })
        }
        other => {
            // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
            return Err(format!(
                "processor type [{other}] is not supported. Available processors: {}",
                PROCESSORS.join(", ")
            ));
        }
    };
    let on_failure = match config.get("on_failure") {
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        None => Vec::new(),
        Some(handlers) if depth < MAX_DEPTH => list(handlers, depth.saturating_add(1))?,
        // ALLOC: compiled once a pipeline a bulk; bounded by the definition (at most 1 MiB).
        Some(_) => return Err("[on_failure] is nested too deeply".to_owned()),
    };
    Ok(Processor {
        step,
        ignore_failure: flag(config, "ignore_failure", false)?,
        on_failure,
    })
}
