//! RFC 6902 patches of security documents: `add`, `remove`, `replace` and
//! `test` over one document, and the bulk form whose paths start with a
//! document name.
//!
//! The patch is applied to the stored document and the result is checked as
//! a `PUT` body is, so a patch can never store what a put would refuse.

use oss_doc::mapping::json::{Json, parse, write_str};

use super::{Answer, Kind, validate};

/// What one operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Add,
    Remove,
    Replace,
    Test,
}

/// One parsed operation: a verb, the path as unescaped tokens, and a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    verb: Verb,
    tokens: Vec<String>,
    value: Option<Json>,
}

fn bad(message: &str) -> Answer {
    // ALLOC: error path; one message.
    let mut text = String::new();
    write_str(&mut text, message);
    Answer {
        status: 400,
        // ALLOC: error path; one message.
        body: format!(r#"{{"status":"BAD_REQUEST","message":{text}}}"#),
    }
}

fn not_found() -> Answer {
    Answer {
        status: 404,
        // ALLOC: error path; one message.
        body: r#"{"status":"NOT_FOUND","message":"Resource not found."}"#.to_owned(),
    }
}

/// The tokens of a JSON pointer: `""` is the document, `/a/b` its member
/// `b` of `a`, with `~1` for `/` and `~0` for `~`.
fn pointer(path: &str) -> Result<Vec<String>, Answer> {
    if path.is_empty() {
        // ALLOC: error path and patch parse; bounded by the 1 MiB body.
        return Ok(Vec::new());
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err(bad("A patch path must be empty or start with '/'."));
    };
    Ok(rest
        .split('/')
        // ALLOC: one token a path segment, bounded by the 1 MiB body.
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        // ALLOC: one token a path segment, bounded by the 1 MiB body.
        .collect())
}

fn compile(item: &Json) -> Result<Op, Answer> {
    let Json::Obj(_) = item else {
        return Err(bad("A patch operation must be an object."));
    };
    let verb = match item.get("op") {
        Some(Json::Str(s)) if s == "add" => Verb::Add,
        Some(Json::Str(s)) if s == "remove" => Verb::Remove,
        Some(Json::Str(s)) if s == "replace" => Verb::Replace,
        Some(Json::Str(s)) if s == "test" => Verb::Test,
        Some(Json::Str(_)) => return Err(bad("Unsupported patch operation.")),
        _ => return Err(bad("A patch operation needs an 'op'.")),
    };
    let Some(Json::Str(path)) = item.get("path") else {
        return Err(bad("A patch operation needs a 'path'."));
    };
    let value = item.get("value").cloned();
    if matches!(verb, Verb::Add | Verb::Replace | Verb::Test) && value.is_none() {
        return Err(bad("A patch operation needs a 'value'."));
    }
    Ok(Op {
        verb,
        tokens: pointer(path)?,
        value,
    })
}

fn parse_ops(body: &[u8]) -> Result<Vec<Op>, Answer> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Err(bad("Request body required for this action."));
    }
    let Ok(Json::Arr(items)) = parse(body) else {
        return Err(bad("A patch must be a JSON array of operations."));
    };
    // ALLOC: one operation each, bounded by the 1 MiB body.
    items.iter().map(compile).collect()
}

fn index(token: &str, len: usize, allow_end: bool) -> Result<usize, Answer> {
    if allow_end && token == "-" {
        return Ok(len);
    }
    let plain = token == "0"
        || (!token.starts_with('0')
            && token.bytes().all(|b| b.is_ascii_digit())
            && !token.is_empty());
    match token.parse::<usize>() {
        Ok(i) if plain && (i < len || (allow_end && i == len)) => Ok(i),
        _ => Err(bad("A patch path names an array index that is not there.")),
    }
}

fn child<'a>(node: &'a mut Json, token: &str) -> Result<&'a mut Json, Answer> {
    match node {
        Json::Obj(members) => members
            .iter_mut()
            .find(|(k, _)| k == token)
            .map(|(_, v)| v)
            .ok_or_else(|| bad("A patch path is not in the document.")),
        Json::Arr(items) => {
            let at = index(token, items.len(), false)?;
            items
                .get_mut(at)
                .ok_or_else(|| bad("A patch path is not in the document."))
        }
        _ => Err(bad(
            "A patch path goes through a value that has no members.",
        )),
    }
}

fn step(root: &mut Json, op: &Op) -> Result<(), Answer> {
    let Some((last, parents)) = op.tokens.split_last() else {
        return Err(bad("unreachable"));
    };
    let mut node = root;
    for token in parents {
        node = child(node, token)?;
    }
    let value = op.value.clone();
    match (op.verb, node) {
        (Verb::Add, Json::Obj(members)) => {
            let value = value.unwrap_or(Json::Null);
            match members.iter_mut().find(|(k, _)| k == last) {
                Some((_, slot)) => *slot = value,
                // ALLOC: one member a patch operation, bounded by the 1 MiB body.
                None => members.push((last.clone(), value)),
            }
            Ok(())
        }
        (Verb::Add, Json::Arr(items)) => {
            let at = index(last, items.len(), true)?;
            items.insert(at, value.unwrap_or(Json::Null));
            Ok(())
        }
        (Verb::Replace, Json::Obj(members)) => {
            let slot = members
                .iter_mut()
                .find(|(k, _)| k == last)
                .ok_or_else(|| bad("A patch path is not in the document."))?;
            slot.1 = value.unwrap_or(Json::Null);
            Ok(())
        }
        (Verb::Replace, Json::Arr(items)) => {
            let at = index(last, items.len(), false)?;
            if let Some(slot) = items.get_mut(at) {
                *slot = value.unwrap_or(Json::Null);
            }
            Ok(())
        }
        (Verb::Remove, Json::Obj(members)) => {
            let before = members.len();
            members.retain(|(k, _)| k != last);
            if members.len() == before {
                return Err(bad("A patch path is not in the document."));
            }
            Ok(())
        }
        (Verb::Remove, Json::Arr(items)) => {
            let at = index(last, items.len(), false)?;
            items.remove(at);
            Ok(())
        }
        (Verb::Test, found) => {
            let found = child(found, last)?;
            if Some(&*found) == value.as_ref() {
                Ok(())
            } else {
                Err(bad("A patch 'test' did not match."))
            }
        }
        _ => Err(bad(
            "A patch path goes through a value that has no members.",
        )),
    }
}

/// Applies `ops` to `doc` (`None` is a document that is not there) and
/// returns the result: `None` when the whole document was removed.
///
/// # Errors
///
/// `400` for a path that is not in the document, a failed `test` or an
/// index that is not there; `404` for an operation inside a document that
/// is not there.
pub fn apply_ops(doc: Option<Json>, ops: &[Op]) -> Result<Option<Json>, Answer> {
    let mut root = doc;
    for op in ops {
        if op.tokens.is_empty() {
            root = match (op.verb, root) {
                (Verb::Add, _) => op.value.clone(),
                (Verb::Replace, Some(_)) => op.value.clone(),
                (Verb::Remove, Some(_)) => None,
                (Verb::Test, Some(found)) => {
                    if Some(&found) != op.value.as_ref() {
                        return Err(bad("A patch 'test' did not match."));
                    }
                    Some(found)
                }
                (_, None) => return Err(not_found()),
            };
        } else {
            let Some(found) = root.as_mut() else {
                return Err(not_found());
            };
            step(found, op)?;
        }
    }
    Ok(root)
}

/// Patches the stored `doc` of `kind` with the RFC 6902 array `body` and
/// checks the result as a `PUT` body is checked.
///
/// # Errors
///
/// The `400` of a patch that is not an array of supported operations, does
/// not apply, or leaves a document `PUT` would refuse.
pub fn patch_document(kind: Kind, doc: Json, body: &[u8]) -> Result<Json, Answer> {
    let ops = parse_ops(body)?;
    let Some(patched) = apply_ops(Some(doc), &ops)? else {
        return Err(bad("A patch cannot remove the document it patches."));
    };
    // ALLOC: one copy of the patched document, bounded by the 1 MiB body.
    validate(kind, patched.to_json().as_bytes())
}

/// Splits a bulk patch by document: `/name` is the whole document and
/// `/name/key` is inside it. Names come in the order first seen; the
/// operations of a name keep their order, with the name taken off the path.
///
/// # Errors
///
/// `400` for anything but an array of `add`, `remove` and `replace`
/// operations whose path starts with a name.
pub fn plan_bulk(body: &[u8]) -> Result<Vec<(String, Vec<Op>)>, Answer> {
    // ALLOC: one entry a distinct name, bounded by the 1 MiB body.
    let mut plan: Vec<(String, Vec<Op>)> = Vec::new();
    for mut op in parse_ops(body)? {
        if op.verb == Verb::Test {
            return Err(bad("A bulk patch does not support 'test'."));
        }
        if op.tokens.first().is_none_or(String::is_empty) {
            return Err(bad("A bulk patch path must start with a name."));
        }
        let name = op.tokens.remove(0);
        match plan.iter_mut().find(|(n, _)| *n == name) {
            Some((_, ops)) => ops.push(op),
            // ALLOC: one entry a distinct name, bounded by the 1 MiB body.
            None => plan.push((name, vec![op])),
        }
    }
    Ok(plan)
}

/// The answer to a bulk patch that was applied.
pub fn bulk_updated() -> Answer {
    Answer {
        status: 200,
        // ALLOC: one answer a request.
        body: r#"{"status":"OK","message":"Resources updated."}"#.to_owned(),
    }
}
