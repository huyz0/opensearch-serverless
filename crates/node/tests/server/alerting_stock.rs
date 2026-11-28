//! docs/specs/alerting AC-14: the monitor routes replayed against the stock
//! 3.10.0 recording (`fixtures/stock-plugins/alerting.json`).

use crate::alerting_routes::{rig, send};

use serde_json::Value;
use std::collections::HashMap;

/// The steps replayed: the monitor's own routes, the legacy prefix and the
/// deletes. The rest need a search, the runner or alerts.
fn replayed(i: usize) -> bool {
    i <= 28 || (55..=68).contains(&i)
}

/// `<volatile>` and `<label>` match anything.
fn is_placeholder(s: &str) -> bool {
    s.starts_with('<') && s.ends_with('>') && !s.contains(' ')
}

/// A label inside longer text stands for the id captured under it.
fn expand(text: &str, labels: &HashMap<String, String>) -> String {
    labels
        .iter()
        .fold(text.to_owned(), |t, (k, v)| t.replace(&format!("<{k}>"), v))
}

fn is_captured(placeholder: &str, labels: &HashMap<String, String>) -> bool {
    placeholder
        .strip_prefix('<')
        .and_then(|p| p.strip_suffix('>'))
        .is_some_and(|label| labels.contains_key(label))
}

fn diff(
    want: &Value,
    got: &Value,
    path: &str,
    labels: &HashMap<String, String>,
    out: &mut Vec<String>,
) {
    match (want, got) {
        (Value::String(w), _) if is_placeholder(w) && !is_captured(w, labels) => {}
        (Value::String(w), Value::String(g)) if w.contains('<') && expand(w, labels) == *g => {}
        (Value::Object(w), Value::Object(g)) => {
            for (k, v) in w {
                match g.get(k) {
                    Some(x) => diff(v, x, &format!("{path}.{k}"), labels, out),
                    None => out.push(format!("{path}.{k}: missing")),
                }
            }
            for k in g.keys().filter(|k| !w.contains_key(*k)) {
                out.push(format!("{path}.{k}: extra"));
            }
        }
        (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
            for (i, (a, b)) in w.iter().zip(g).enumerate() {
                diff(a, b, &format!("{path}[{i}]"), labels, out);
            }
        }
        _ if want == got => {}
        _ => out.push(format!("{path}: wanted {want}, got {got}")),
    }
}

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/stock-plugins/alerting.json"
    );
    let file = std::fs::File::open(path).expect("fixture");
    serde_json::from_reader(std::io::BufReader::new(file)).expect("json")
}

/// The id a step's answer carries under a `<label>` in the recording: later
/// steps write it as `{label}`.
fn captured(step: &Value, got: &Value) -> Option<(String, String)> {
    let want = step["response"]["body"]["_id"].as_str()?;
    let label = want.strip_prefix('<')?.strip_suffix('>')?;
    Some((label.to_owned(), got["_id"].as_str()?.to_owned()))
}

/// Every difference between the recording and this node, by step.
pub(crate) fn differences() -> Vec<String> {
    let r = rig();
    let recording = fixture();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    let fill = |text: &str, labels: &HashMap<String, String>| {
        labels.iter().fold(text.to_owned(), |t, (k, v)| {
            t.replace(&format!("{{{k}}}"), v)
        })
    };
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
        let mut target = fill(req["path"].as_str().expect("path"), &labels);
        if let Some(q) = req["query"].as_object() {
            let pairs: Vec<String> = q
                .iter()
                .map(|(k, v)| format!("{k}={}", fill(v.as_str().unwrap_or_default(), &labels)))
                .collect();
            target = format!("{target}?{}", pairs.join("&"));
        }
        let body = req.get("body").map(|b| fill(&b.to_string(), &labels));
        let (status, got) = send(
            r.server.local_addr(),
            method,
            &target,
            None,
            body.as_deref(),
        );
        let name = step["name"].as_str().unwrap_or_default();
        let want = &step["response"];
        let want_status = want["status"].as_u64().unwrap_or(0);
        if u64::from(status) != want_status {
            out.push(format!(
                "step {i} [{name}]: status wanted {want_status}, got {status}"
            ));
        }
        if method != "HEAD" {
            let mut d = Vec::new();
            diff(&want["body"], &got, "", &labels, &mut d);
            out.extend(d.into_iter().map(|x| format!("step {i} [{name}]: {x}")));
        }
        if let Some((label, value)) = captured(step, &got) {
            labels.insert(label, value);
        }
    }
    out
}

/// The steps that still differ, each for a reason the spec lists as a gap.
const GAPS: &[usize] = &[];

// AC-14
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
