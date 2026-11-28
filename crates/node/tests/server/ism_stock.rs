//! docs/specs/ism AC-13: the routes replayed against the stock 3.10.0
//! recording (`fixtures/stock-plugins/ism.json`).

use crate::ism_rig::call;
use crate::ism_template::rig;

use serde_json::Value;

/// The steps replayed: everything before the runner steps, and the legacy route.
fn replayed(i: usize) -> bool {
    i <= 56 || i == 67
}

/// `<volatile>` and `<label>` match anything; `<n>` inside a text matches digits.
fn text_matches(want: &str, got: &str) -> bool {
    if want.starts_with('<') && want.ends_with('>') && !want.contains(' ') {
        return true;
    }
    let mut rest = got;
    let mut parts = want.split("<n>").peekable();
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(after) = rest.strip_prefix(first) else {
        return false;
    };
    rest = after;
    while let Some(part) = parts.next() {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        let Some(after) = rest.strip_prefix(part) else {
            return false;
        };
        rest = after;
        if parts.peek().is_none() {
            return rest.is_empty();
        }
    }
    rest.is_empty()
}

fn diff(want: &Value, got: &Value, path: &str, out: &mut Vec<String>) {
    match (want, got) {
        (Value::String(w), Value::String(g)) if text_matches(w, g) => {}
        (Value::String(w), _) if w.starts_with('<') && w.ends_with('>') && !w.contains(' ') => {
            if got.is_null() && w == "<volatile>" {
                out.push(format!("{path}: wanted a value, got null"));
            }
        }
        (Value::Object(w), Value::Object(g)) => {
            for (k, v) in w {
                match g.get(k) {
                    Some(x) => diff(v, x, &format!("{path}.{k}"), out),
                    None => out.push(format!("{path}.{k}: missing")),
                }
            }
            for k in g.keys().filter(|k| !w.contains_key(*k)) {
                out.push(format!("{path}.{k}: extra"));
            }
        }
        (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
            for (i, (a, b)) in w.iter().zip(g).enumerate() {
                diff(a, b, &format!("{path}[{i}]"), out);
            }
        }
        _ if want == got => {}
        _ => out.push(format!("{path}: wanted {want}, got {got}")),
    }
}

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/stock-plugins/ism.json"
    );
    let file = std::fs::File::open(path).expect("fixture");
    serde_json::from_reader(std::io::BufReader::new(file)).expect("json")
}

/// Every difference between the recording and this node, by step.
pub(crate) fn differences() -> Vec<String> {
    let r = rig(true);
    let recording = fixture();
    let (mut seq, mut term) = (Value::Null, Value::Null);
    let mut out = Vec::new();
    for (i, step) in recording["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .enumerate()
    {
        if !replayed(i) {
            continue;
        }
        let req = &step["request"];
        let method = req["method"].as_str().expect("method");
        let mut target = req["path"].as_str().expect("path").to_owned();
        if let Some(q) = req["query"].as_object() {
            let pairs: Vec<String> = q
                .iter()
                .map(|(k, v)| {
                    let v = match v.as_str() {
                        Some("{seq}") => seq.to_string(),
                        Some("{term}") => term.to_string(),
                        Some(s) => s.to_owned(),
                        None => v.to_string(),
                    };
                    format!("{k}={}", v.replace('*', "%2A"))
                })
                .collect();
            target = format!("{target}?{}", pairs.join("&"));
        }
        let body = match (req.get("body"), method) {
            (Some(b), _) => Some(b.to_string()),
            (None, "PUT") if !target.contains("_ism") => Some("{}".to_owned()),
            _ => None,
        };
        let got = call(&r, method, &target, body.as_deref());
        let name = step["name"].as_str().unwrap_or_default();
        let want = &step["response"];
        let want_status = want["status"].as_u64().unwrap_or(0);
        if u64::from(got.status) != want_status {
            out.push(format!(
                "step {i} [{name}]: status wanted {want_status}, got {}",
                got.status
            ));
        }
        let got_body: Value = serde_json::from_slice(&got.body).unwrap_or(Value::Null);
        if method != "HEAD" {
            let mut d = Vec::new();
            diff(&want["body"], &got_body, "", &mut d);
            out.extend(d.into_iter().map(|x| format!("step {i} [{name}]: {x}")));
        }
        if name == "get policy" || name == "put policy" {
            seq = got_body["_seq_no"].clone();
            term = got_body["_primary_term"].clone();
        }
    }
    out
}

/// The steps that still differ, each for a reason the spec lists as a gap.
///
/// - 11, 43: the plugin's search error also carries `failed_shards` and `grouped`
///   (43 also: a wildcard is not served, since resolving one would list).
/// - 23: the overlap answer has no `caused_by`, since the error body takes its
///   root cause from the innermost cause.
/// - 20, 21, 26, 27: the recording's policy holds actions this node refuses at put
///   (`force_merge`, `close`, `open`, `shrink`, `allocation`, `index_priority`,
///   `notification`, size conditions), so the policy is not stored.
/// - 25, 28, 29: the create answer of the index route also names `shards`.
/// - 32: a wildcard is not served (resolving one would list).
const GAPS: &[usize] = &[11, 20, 21, 23, 25, 26, 27, 28, 29, 32, 43];

// AC-13
#[test]
fn the_stock_recording_replays_with_only_the_listed_gaps() {
    let found = differences();
    let mut steps: Vec<usize> = found
        .iter()
        .filter_map(|d| d.strip_prefix("step ")?.split(' ').next()?.parse().ok())
        .collect();
    steps.dedup();
    assert_eq!(steps, GAPS, "{}", found.join("\n"));
}
