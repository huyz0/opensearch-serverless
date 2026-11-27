use oss_doc::mapping::json::{Json, java_double};

use super::{Processor, Step, TextOp, Type};

pub(super) fn run_all(processors: &[Processor], doc: &mut Json) -> Result<(), String> {
    for p in processors {
        if let Err(reason) = step(&p.step, doc) {
            if !p.on_failure.is_empty() {
                run_all(&p.on_failure, doc)?;
            } else if !p.ignore_failure {
                return Err(reason);
            }
        }
    }
    Ok(())
}

fn path(field: &str) -> impl Iterator<Item = &str> {
    field.split('.')
}

fn find<'a>(doc: &'a Json, field: &str) -> Option<&'a Json> {
    path(field).try_fold(doc, |at, key| at.get(key))
}

fn find_mut<'a>(doc: &'a mut Json, field: &str) -> Option<&'a mut Json> {
    path(field).try_fold(doc, |at, key| match at {
        Json::Obj(members) => members.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    })
}

fn put(doc: &mut Json, field: &str, value: Json) -> Result<(), String> {
    // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
    let keys: Vec<&str> = path(field).collect();
    let Some((last, parents)) = keys.split_last() else {
        // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
        return Err(format!("cannot set [{field}]"));
    };
    let mut at = doc;
    for key in parents {
        let Json::Obj(members) = at else {
            // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
            return Err(format!("cannot set [{field}]: [{key}] is not an object"));
        };
        let i = match members.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                members.push(((*key).to_owned(), Json::Obj(Vec::new())));
                members.len().saturating_sub(1)
            }
        };
        let Some((_, next)) = members.get_mut(i) else {
            // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
            return Err(format!("cannot set [{field}]"));
        };
        at = next;
    }
    let Json::Obj(members) = at else {
        // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
        return Err(format!("cannot set [{field}]: the parent is not an object"));
    };
    match members.iter_mut().find(|(k, _)| k == last) {
        Some(slot) => slot.1 = value,
        // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
        None => members.push(((*last).to_owned(), value)),
    }
    Ok(())
}

fn take(doc: &mut Json, field: &str) -> Option<Json> {
    let (parent, last) = match field.rsplit_once('.') {
        Some((parent, last)) => (find_mut(doc, parent)?, last),
        None => (doc, field),
    };
    let Json::Obj(members) = parent else {
        return None;
    };
    let i = members.iter().position(|(k, _)| k == last)?;
    Some(members.remove(i).1)
}

fn missing(field: &str) -> String {
    // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
    format!("field [{field}] not present as part of path [{field}]")
}

fn kind(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "java.lang.Boolean",
        Json::Num(n) if oss_doc::mapping::json::is_integral(n) => "java.lang.Integer",
        Json::Num(_) => "java.lang.Double",
        Json::Str(_) => "java.lang.String",
        Json::Arr(_) => "java.util.ArrayList",
        Json::Obj(_) => "java.util.LinkedHashMap",
    }
}

fn step(s: &Step, doc: &mut Json) -> Result<(), String> {
    match s {
        Step::Set {
            field,
            value,
            overwrite,
            ignore_empty,
        } => {
            let empty =
                matches!(value, Json::Null) || matches!(value, Json::Str(t) if t.is_empty());
            if (*ignore_empty && empty)
                || (!*overwrite && find(doc, field).is_some_and(|v| *v != Json::Null))
            {
                return Ok(());
            }
            put(doc, field, value.clone())
        }
        Step::Remove {
            fields,
            ignore_missing,
        } => {
            for field in fields {
                if take(doc, field).is_none() && !*ignore_missing {
                    return Err(missing(field));
                }
            }
            Ok(())
        }
        Step::Rename {
            field,
            target,
            ignore_missing,
        } => {
            if find(doc, field).is_none() {
                return if *ignore_missing {
                    Ok(())
                } else {
                    Err(missing(field))
                };
            }
            if find(doc, target).is_some() {
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                return Err(format!("field [{target}] already exists"));
            }
            let value = take(doc, field).ok_or_else(|| missing(field))?;
            put(doc, target, value)
        }
        Step::Text {
            field,
            target,
            ignore_missing,
            op,
        } => {
            let Some(value) = find(doc, field) else {
                return if *ignore_missing {
                    Ok(())
                } else {
                    Err(missing(field))
                };
            };
            let Json::Str(s) = value else {
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                return Err(format!(
                    "field [{field}] of type [{}] cannot be cast to [java.lang.String]",
                    kind(value)
                ));
            };
            let out = match op {
                TextOp::Lower => s.to_lowercase(),
                TextOp::Upper => s.to_uppercase(),
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                TextOp::Trim => s.trim().to_owned(),
            };
            put(doc, target, Json::Str(out))
        }
        Step::Append {
            field,
            values,
            allow_duplicates,
        } => {
            let mut items = match find(doc, field) {
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                None | Some(Json::Null) => Vec::new(),
                Some(Json::Arr(items)) => items.clone(),
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                Some(one) => vec![one.clone()],
            };
            for v in values {
                if *allow_duplicates || !items.contains(v) {
                    items.push(v.clone());
                }
            }
            put(doc, field, Json::Arr(items))
        }
        Step::Convert {
            field,
            target,
            ignore_missing,
            to,
        } => {
            let Some(value) = find(doc, field) else {
                return if *ignore_missing {
                    Ok(())
                } else {
                    Err(missing(field))
                };
            };
            let converted = match value {
                Json::Arr(items) => Json::Arr(
                    items
                        .iter()
                        .map(|i| convert(i, *to))
                        .collect::<Result<_, _>>()?,
                ),
                one => convert(one, *to)?,
            };
            put(doc, target, converted)
        }
    }
}

fn convert(value: &Json, to: Type) -> Result<Json, String> {
    let said = match value {
        Json::Str(s) => s.clone(),
        Json::Num(n) => n.clone(),
        // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
        Json::Bool(b) => b.to_string(),
        // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
        other => return Err(format!("unable to convert [{}] to {to:?}", other.to_json())),
    };
    // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
    let fail = |what: &str| format!("unable to convert [{said}] to {what}");
    Ok(match to {
        Type::Integer => Json::Num(
            said.trim()
                .parse::<i32>()
                .map_err(|_| fail("integer"))?
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                .to_string(),
        ),
        Type::Long => Json::Num(
            said.trim()
                .parse::<i64>()
                .map_err(|_| fail("long"))?
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                .to_string(),
        ),
        Type::Float | Type::Double => {
            let v = said.trim().parse::<f64>().map_err(|_| fail("double"))?;
            if !v.is_finite() {
                return Err(fail("double"));
            }
            Json::Num(java_double(v))
        }
        Type::Boolean => match said.as_str() {
            "true" => Json::Bool(true),
            "false" => Json::Bool(false),
            _ => {
                // ALLOC: one document's working copy or one message; bounded by the source (at most the body limit).
                return Err(format!(
                    "[{said}] is not a boolean value, cannot convert to boolean"
                ));
            }
        },
        Type::Text => Json::Str(said),
    })
}
