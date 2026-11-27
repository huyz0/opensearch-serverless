//! Scoring a hit with a learning to rank model allocates nothing
//! (docs/specs/ltr/SPEC.md, AC-13), for every evaluator. Its own binary,
//! because the counting allocator is process-wide.

// ARITH: test sizes are small constants; an overflow panics the test, which
// fails it as it should.
#![allow(clippy::arithmetic_side_effects)]

use oss_reader::ltr::Evaluator;
use oss_testkit::{CountingAllocator, PeakWindow, counting_allocator};

#[global_allocator]
static ALLOC: CountingAllocator = counting_allocator();

const XGBOOST: &str = include_str!("../../../fixtures/ltr/xgboost-binary-logistic-model.json");
const RANKLIB: &str = "## LambdaMART\n## No. of trees = 2\n## Learning rate = 0.1\n\n<ensemble>\n\
 <tree id=\"1\" weight=\"0.5\">\n  <split>\n   <feature> 1 </feature>\n   <threshold> 0.5 </threshold>\n\
   <split pos=\"left\">\n    <output> -1.0 </output>\n   </split>\n\
   <split pos=\"right\">\n    <output> 4.0 </output>\n   </split>\n  </split>\n </tree>\n\
 <tree id=\"2\" weight=\"2.0\">\n  <split>\n   <feature> 2 </feature>\n   <threshold> 1.0 </threshold>\n\
   <split pos=\"left\">\n    <output> 0.25 </output>\n   </split>\n\
   <split pos=\"right\">\n    <output> 0.75 </output>\n   </split>\n  </split>\n </tree>\n</ensemble>\n";
const FOREST: &str = "## Random Forests\n## No. of bags = 2\n<ensemble><tree id=\"1\" weight=\"1.0\"><split><output> 2.0 </output></split></tree></ensemble><ensemble><tree id=\"1\" weight=\"1.0\"><split><output> 4.0 </output></split></tree></ensemble>";

#[test]
fn scoring_a_hit_allocates_nothing() {
    let names = |n: usize| -> Vec<String> { (1..=n).map(|i| format!("f{i}")).collect() };
    let models = [
        ("model/linear", r#"{"f1":2.0,"f2":-1.0}"#, 2),
        ("model/xgboost+json", XGBOOST, 4),
        ("model/ranklib", RANKLIB, 2),
        ("model/ranklib", FOREST, 1),
    ]
    .map(|(kind, definition, n)| {
        Evaluator::parse(kind, definition, &names(n)).expect("the model parses")
    });
    let vectors: [&[f32]; 3] = [
        &[0.1, 0.9, 0.3, 0.5],
        &[f32::NAN, 0.5, f32::NAN, 0.5],
        &[0.7, 2.0, 0.1, f32::NAN],
    ];
    let window = PeakWindow::start(&ALLOC);
    let mut sum = 0.0f32;
    for _ in 0..1_000 {
        for m in &models {
            for v in vectors {
                sum += m.score(v);
            }
        }
    }
    assert_eq!(
        window.allocated_bytes(),
        0,
        "scoring 12,000 hits called the allocator"
    );
    assert!(sum.is_finite());
}
