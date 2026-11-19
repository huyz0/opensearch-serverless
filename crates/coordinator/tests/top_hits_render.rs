//! docs/specs/coordinator-top-hits-render, AC-5 to AC-7: the merged page's
//! hits are fetched from their shards and written in page order.

use std::sync::Mutex;

use oss_coordinator::reduce::top_hits::{RenderError, TopHitsMerged, render_page};

fn merged(page: &[(usize, usize)], total: u64, max_score: Option<f32>) -> TopHitsMerged {
    TopHitsMerged {
        total,
        max_score,
        page: page.to_vec(),
        peak_held: 0,
    }
}

/// A fetch that answers `s<shard>p<position>` for each position and records
/// the calls it was given.
fn fetch(
    calls: &Mutex<Vec<(usize, Vec<usize>)>>,
    shard: usize,
    positions: &[usize],
) -> Result<Vec<Vec<u8>>, String> {
    calls
        .lock()
        .expect("lock")
        .push((shard, positions.to_vec()));
    Ok(positions
        .iter()
        .map(|p| format!("{{\"hit\":\"s{shard}p{p}\"}}").into_bytes())
        .collect())
}

#[test]
fn hits_are_written_in_page_order_with_one_call_a_shard() {
    let calls = Mutex::new(Vec::new());
    let page = merged(&[(1, 0), (0, 2), (1, 1), (0, 0)], 42, Some(2.5));
    let body =
        render_page(&page, 4, |shard, positions| fetch(&calls, shard, positions)).expect("renders");
    assert_eq!(
        String::from_utf8(body).expect("utf-8"),
        concat!(
            r#""hits":{"total":{"value":42,"relation":"eq"},"max_score":2.5,"hits":["#,
            r#"{"hit":"s1p0"},{"hit":"s0p2"},{"hit":"s1p1"},{"hit":"s0p0"}]}"#
        )
    );
    let mut calls = calls.into_inner().expect("lock");
    calls.sort();
    assert_eq!(calls, [(0, vec![2, 0]), (1, vec![0, 1])]);
}

#[test]
fn no_score_is_written_as_null() {
    let page = merged(&[(0, 0)], 1, None);
    let body = render_page(&page, 1, |_, p| {
        Ok::<_, String>(p.iter().map(|_| b"{}".to_vec()).collect())
    })
    .expect("renders");
    let body = String::from_utf8(body).expect("utf-8");
    assert!(body.contains(r#""max_score":null"#), "{body}");
}

#[test]
fn a_failing_shard_fails_the_render_and_is_named() {
    let page = merged(&[(0, 0), (2, 0)], 2, Some(1.0));
    let err = render_page(&page, 2, |shard, p| {
        if shard == 2 {
            Err("gone".to_string())
        } else {
            Ok(p.iter().map(|_| b"{}".to_vec()).collect())
        }
    })
    .expect_err("fails");
    assert_eq!(
        err,
        RenderError::Failed {
            shard: 2,
            error: "gone".to_string()
        }
    );
}

#[test]
fn an_answer_of_the_wrong_length_fails_the_render() {
    let page = merged(&[(0, 0), (0, 1)], 2, Some(1.0));
    let err =
        render_page(&page, 1, |_, _| Ok::<_, String>(vec![b"{}".to_vec()])).expect_err("fails");
    assert_eq!(
        err,
        RenderError::WrongCount {
            shard: 0,
            asked: 2,
            got: 1
        }
    );
}

#[test]
fn an_empty_page_writes_an_empty_list_without_asking_anyone() {
    let calls = Mutex::new(Vec::new());
    let page = merged(&[], 7, None);
    let body = render_page(&page, 4, |shard, p| fetch(&calls, shard, p)).expect("renders");
    assert_eq!(
        String::from_utf8(body).expect("utf-8"),
        r#""hits":{"total":{"value":7,"relation":"eq"},"max_score":null,"hits":[]}"#
    );
    assert!(calls.into_inner().expect("lock").is_empty());
}

#[test]
fn a_panicking_fetch_fails_the_render_and_names_its_shard() {
    let page = merged(&[(3, 0)], 1, None);
    let err = render_page(&page, 1, |_, _| -> Result<Vec<Vec<u8>>, String> {
        panic!("fetch panicked")
    })
    .expect_err("fails");
    assert_eq!(err, RenderError::Panicked { shard: 3 });
}
