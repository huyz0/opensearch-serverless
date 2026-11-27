//! docs/specs/ltr slice 3: an `sltr` rescore over the compat index, with a
//! model resolver standing in for the node's store.

use std::sync::{Arc, Once};

use crate::support::{request, start};
use oss_reader::ltr::Evaluator;
use oss_reader::ltr::models::{Feature, FeatureSet, Models, ResolveError, Resolved, install};

const ANIMAL: &str = r#"{"constant_score":{"filter":{"term":{"tag":"animal"}},"boost":5}}"#;
const FOOD: &str = r#"{"constant_score":{"filter":{"term":{"tag":"food"}},"boost":{{w}}}}"#;

pub(crate) struct Fake;

fn features() -> Vec<Feature> {
    vec![
        Feature {
            name: "animal".into(),
            params: vec![],
            template: ANIMAL.into(),
        },
        Feature {
            name: "food".into(),
            params: vec!["w".into()],
            template: FOOD.into(),
        },
    ]
}

impl Models for Fake {
    fn resolve(&self, store: Option<&str>, name: &str) -> Result<Arc<Resolved>, ResolveError> {
        if store.is_some() || name != "lin" {
            return Err(ResolveError::NotFound(format!(
                "model [{name}] does not exist"
            )));
        }
        let features = features();
        let names: Vec<String> = features.iter().map(|f| f.name.clone()).collect();
        let evaluator = Evaluator::parse("model/linear", r#"{"animal":2.0,"food":1.0}"#, &names)
            .expect("parses");
        Ok(Arc::new(Resolved {
            evaluator,
            features,
        }))
    }

    fn feature_set(&self, _: Option<&str>, name: &str) -> Result<Arc<FeatureSet>, ResolveError> {
        if name != "fs" {
            return Err(ResolveError::NotFound(format!(
                "feature set [{name}] does not exist"
            )));
        }
        Ok(Arc::new(FeatureSet {
            features: features(),
        }))
    }

    fn max_window(&self) -> usize {
        100
    }
}

fn run(inner: &str, window: u32) -> (u16, String) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| install(Arc::new(Fake)));
    let node = start();
    let body = format!(
        r#"{{"size":10,"query":{{"constant_score":{{"filter":{{"match_all":{{}}}},"boost":2}}}},"rescore":{{"window_size":{window},"query":{{{inner}}}}}}}"#
    );
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    (r.status, r.text())
}

fn scores(body: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find(r#""_id":""#) {
        rest = &rest[i + 7..];
        let id = rest[..rest.find('"').expect("id")].to_owned();
        let j = rest.find(r#""_score":"#).expect("score") + 9;
        let end = rest[j..].find([',', '}']).expect("end") + j;
        out.push((id, rest[j..end].parse().expect("a number")));
    }
    out
}

fn sltr(extra: &str) -> String {
    format!(
        r#""rescore_query":{{"sltr":{{"model":"lin","params":{{"w":3}}{extra}}}}},"query_weight":0"#
    )
}

// AC-8
#[test]
fn an_sltr_rescore_reorders_the_window_by_the_models_score() {
    let (status, body) = run(&sltr(""), 10);
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    // animal documents 2 x 5; the food one 3 (w); the manual one matches no feature
    for (id, score) in &got {
        let want = match id.as_str() {
            "1" | "2" | "5" => 10.0,
            "4" => 3.0,
            _ => 0.0,
        };
        assert_eq!(*score, want, "{id}: {body}");
    }
    assert!(got[..3].iter().all(|(_, s)| *s == 10.0), "{body}");
}

// AC-8: hits outside the window keep the first pass's score and order
#[test]
fn hits_outside_the_window_keep_their_first_pass_order() {
    let inner = r#""rescore_query":{"sltr":{"model":"lin","params":{"w":3}}},"query_weight":1"#;
    let (status, body) = run(inner, 2);
    assert_eq!(status, 200, "{body}");
    let got = scores(&body);
    assert_eq!(got.len(), 5, "{body}");
    // two rescored hits lead (2 + model), the other three keep 2
    assert!(got[0].1 > 2.0 && got[1].1 > 2.0, "{body}");
    assert!(got[2..].iter().all(|(_, s)| *s == 2.0), "{body}");
}

// AC-9
#[test]
fn params_reach_the_templates_and_an_inactive_feature_scores_zero() {
    let (_, body) = run(&sltr(r#","active_features":["animal"]"#), 10);
    for (id, score) in scores(&body) {
        let want = if ["1", "2", "5"].contains(&id.as_str()) {
            10.0
        } else {
            0.0
        };
        assert_eq!(score, want, "{id}: {body}");
    }
    let no_param = r#""rescore_query":{"sltr":{"model":"lin"}},"query_weight":0"#;
    let (status, body) = run(no_param, 10);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("missing required param [w]"), "{body}");
}

// AC-12
#[test]
fn a_missing_model_is_404_an_unknown_feature_400_and_a_wide_window_400() {
    let missing = r#""rescore_query":{"sltr":{"model":"nope","params":{"w":1}}}"#;
    let (status, body) = run(missing, 10);
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("resource_not_found_exception"), "{body}");
    let (status, body) = run(&sltr(r#","active_features":["nope"]"#), 10);
    assert_eq!(status, 400, "{body}");
    let (status, body) = run(&sltr(""), 101);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("max_window"), "{body}");
}

#[test]
fn an_sltr_query_anywhere_but_a_rescore_is_refused() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| install(Arc::new(Fake)));
    let node = start();
    let body = r#"{"query":{"sltr":{"model":"lin","params":{"w":1}}}}"#;
    let r = request(
        node.addr(),
        "POST",
        "/compat-term/_search",
        Some(body.as_bytes()),
    );
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(r.text().contains("rescore_query"), "{}", r.text());
}
