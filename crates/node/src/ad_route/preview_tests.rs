use oss_anomaly::Params;
use serde_json::json;

use super::{MAX_PREVIEW_POINTS, params, plan, results, score};
use crate::ad_route::windows::{Feature, Span};

fn series(n: usize, spike_at: Option<usize>) -> Vec<Option<Vec<f32>>> {
    (0..n)
        .map(|i| {
            let v = (i % 10) as f32;
            Some(vec![if Some(i) == spike_at { 500.0 } else { v }])
        })
        .collect()
}

#[test]
fn the_history_is_what_the_model_needs_cut_back_to_the_bound() {
    let p = Params::new(1, 0);
    let big = plan(10_000_000, 60_000, 100, &p).unwrap();
    assert_eq!(big.history, p.min_train_samples + p.shingle_size);
    assert_eq!(big.all.count, big.history + 100);
    assert_eq!(big.all.from_ms, 10_000_000 - big.history as i64 * 60_000);
    let tight = plan(10_000_000, 60_000, MAX_PREVIEW_POINTS - 10, &p).unwrap();
    assert_eq!((tight.history, tight.all.count), (10, MAX_PREVIEW_POINTS));
    let full = plan(0, 60_000, MAX_PREVIEW_POINTS, &p).unwrap();
    assert_eq!((full.history, full.all.from_ms), (0, 0));
    assert!(
        plan(0, 60_000, MAX_PREVIEW_POINTS + 1, &p)
            .unwrap_err()
            .contains("1 to 1000")
    );
    assert!(plan(0, 60_000, 0, &p).is_err());
}

#[test]
fn a_preview_flags_the_spike_and_grades_the_steady_windows_zero() {
    let detector = json!({"shingle_size": 4});
    let p = params(&detector, 1).unwrap();
    let history = p.min_train_samples + p.shingle_size;
    let mut windows = series(history + 60, Some(history + 40));
    // the spike is in the span; a windows before it is steady
    let scored = score(p.clone(), &windows, history).unwrap();
    assert_eq!(scored.len(), 60);
    let grade = |i: usize| scored[i].map_or(0.0, |s| s.grade);
    assert!(grade(40) > 0.0, "the spike is flagged");
    assert!((0..39).all(|i| grade(i) < 0.5), "steady windows are not");
    // the same windows give the same grades
    assert_eq!(score(p, &windows, history).unwrap(), scored);
    windows.clear();
}

#[test]
fn the_result_names_each_window_and_its_features_and_is_empty_without_data() {
    let features = [Feature {
        id: "f1".into(),
        name: "cpu".into(),
        agg: json!({}),
    }];
    let span = Span {
        from_ms: 1_000,
        interval_ms: 500,
        count: 2,
    };
    let values = vec![Some(vec![3.0]), None];
    let out = results(&features, &span, &values, &[None, None], None);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["data_start_time"], 1_000);
    assert_eq!(out[0]["data_end_time"], 1_500);
    assert_eq!(
        out[0]["feature_data"][0],
        json!({"feature_id": "f1", "feature_name": "cpu", "data": 3.0})
    );
    assert_eq!(out[1]["feature_data"][0]["data"], json!(null));
    assert_eq!(out[1]["anomaly_grade"], 0.0);
    assert!(results(&features, &span, &[None, None], &[None, None], None).is_empty());
}

#[test]
fn a_value_the_model_refuses_names_its_window() {
    let p = params(&json!({"shingle_size": 2}), 1).unwrap();
    let err = score(p, &[Some(vec![1.0]), Some(vec![f32::INFINITY])], 0).unwrap_err();
    assert!(err.starts_with("window 1: "), "{err}");
}

#[test]
fn an_entity_result_carries_its_key() {
    let features = [Feature {
        id: "f1".into(),
        name: "cpu".into(),
        agg: json!({}),
    }];
    let span = Span {
        from_ms: 0,
        interval_ms: 500,
        count: 1,
    };
    let key = [("host".to_owned(), "a".to_owned())];
    let out = results(&features, &span, &[Some(vec![1.0])], &[None], Some(&key));
    assert_eq!(out[0]["entity"], json!([{"name": "host", "value": "a"}]));
    let plain = results(&features, &span, &[Some(vec![1.0])], &[None], None);
    assert!(plain[0].get("entity").is_none());
}
