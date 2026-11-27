//! Evaluator tests: hand-built models and recorded XGBoost scores.

// ARITH: test sizes are small constants; an overflow panics the test, which fails
// it as it should. The weighted sums are written as the model computes them.
#![allow(clippy::arithmetic_side_effects, clippy::neg_multiply)]

use super::{Evaluator, MAX_DEPTH, MAX_TREES};

fn names(n: usize) -> Vec<String> {
    // ALLOC: test data.
    (1..=n).map(|i| format!("f{i}")).collect()
}

fn parse(kind: &str, definition: &str, features: usize) -> Result<Evaluator, super::ModelError> {
    Evaluator::parse(kind, definition, &names(features))
}

fn close(got: f32, want: f32) -> bool {
    (got - want).abs() <= 1e-5 * want.abs().max(1.0)
}

#[test]
fn a_linear_model_scores_the_dot_product_of_its_weights() {
    let m = parse("model/linear", r#"{"f1":2.0,"f3":-0.5}"#, 3).expect("parses");
    assert!(close(m.score(&[1.0, 100.0, 4.0]), 0.0));
    assert!(close(m.score(&[1.5, 0.0, 2.0]), 2.0));
    // a feature that did not match counts 0
    assert!(close(m.score(&[f32::NAN, 1.0, 2.0]), -1.0));
}

#[test]
fn a_linear_model_naming_a_feature_the_set_lacks_is_refused() {
    let e = parse("model/linear", r#"{"nope":1.0}"#, 2).expect_err("refused");
    assert!(e.reason().contains("[nope]"), "{e}");
}

#[test]
fn a_malformed_definition_is_refused_with_its_position() {
    let e = parse("model/linear", "{\"f1\":\n  oops}", 1).expect_err("refused");
    assert!(e.reason().contains("line: 2"), "{e}");
    assert!(parse("model/linear", "[1]", 1).is_err());
    assert!(parse("model/linear", r#"{"f1":"x"}"#, 1).is_err());
    assert!(parse("model/mystery", "{}", 1).is_err());
}

fn fixture(objective: &str) -> (String, &'static str) {
    let model = match objective {
        "reg-squarederror" => {
            include_str!("../../../../fixtures/ltr/xgboost-reg-squarederror-model.json")
        }
        "reg-logistic" => include_str!("../../../../fixtures/ltr/xgboost-reg-logistic-model.json"),
        _ => include_str!("../../../../fixtures/ltr/xgboost-binary-logistic-model.json"),
    };
    let scores = match objective {
        "reg-squarederror" => {
            include_str!("../../../../fixtures/ltr/xgboost-reg-squarederror-scores.txt")
        }
        "reg-logistic" => include_str!("../../../../fixtures/ltr/xgboost-reg-logistic-scores.txt"),
        _ => include_str!("../../../../fixtures/ltr/xgboost-binary-logistic-scores.txt"),
    };
    // ALLOC: test data.
    (model.to_owned(), scores)
}

fn check_recorded(objective: &str) {
    let (model, scores) = fixture(objective);
    let m = parse("model/xgboost+json", &model, 4).expect("parses");
    let mut rows = 0;
    for line in scores.lines() {
        let (vector, want) = line.split_once(" -> ").expect("a row");
        let vector: Vec<f32> = vector
            .split(' ')
            .map(|v| {
                if v == "nan" {
                    f32::NAN
                } else {
                    v.parse().expect("number")
                }
            })
            // ALLOC: test data.
            .collect();
        let want: f32 = want.parse().expect("score");
        let got = m.score(&vector);
        assert!(
            close(got, want),
            "{objective}: {vector:?} scored {got}, XGBoost {want}"
        );
        rows += 1;
    }
    assert_eq!(rows, 8);
}

#[test]
fn xgboost_squared_error_scores_as_xgboost_does() {
    check_recorded("reg-squarederror");
}

#[test]
fn xgboost_reg_logistic_scores_as_xgboost_does() {
    check_recorded("reg-logistic");
}

#[test]
fn xgboost_binary_logistic_scores_as_xgboost_does() {
    check_recorded("binary-logistic");
}

fn stump(split: &str) -> String {
    // ALLOC: test data.
    format!(
        r#"{{"learner":{{"learner_model_param":{{"base_score":"5E-1"}},"objective":{{"name":"reg:squarederror"}},"gradient_booster":{{"name":"gbtree","model":{{"trees":[{split}]}}}}}}}}"#
    )
}

const STUMP: &str = r#"{"left_children":[1,-1,-1],"right_children":[2,-1,-1],"split_indices":[0,0,0],"split_conditions":[0.5,-1.0,2.0],"default_left":[0,0,0]}"#;

#[test]
fn xgboost_a_missing_value_follows_its_default_direction() {
    let left = STUMP.replace("\"default_left\":[0,0,0]", "\"default_left\":[1,0,0]");
    let go_left = parse("model/xgboost+json", &stump(&left), 1).expect("parses");
    let go_right = parse("model/xgboost+json", &stump(STUMP), 1).expect("parses");
    assert!(close(go_left.score(&[f32::NAN]), -0.5));
    assert!(close(go_right.score(&[f32::NAN]), 2.5));
    // below the threshold is left, at it is right
    assert!(close(go_right.score(&[0.49]), -0.5));
    assert!(close(go_right.score(&[0.5]), 2.5));
}

#[test]
fn xgboost_other_objectives_and_boosters_are_refused() {
    let m = stump(STUMP);
    let e = parse(
        "model/xgboost+json",
        &m.replace("reg:squarederror", "multi:softprob"),
        1,
    )
    .expect_err("refused");
    assert!(e.reason().contains("multi:softprob"), "{e}");
    assert!(parse("model/xgboost+json", &m.replace("gbtree", "dart"), 1).is_err());
    assert!(parse("model/xgboost+json", "{}", 1).is_err());
}

#[test]
fn xgboost_a_split_on_a_feature_the_set_lacks_is_refused() {
    let m = stump(&STUMP.replace("\"split_indices\":[0,0,0]", "\"split_indices\":[3,0,0]"));
    assert!(parse("model/xgboost+json", &m, 2).is_err());
}

#[test]
fn xgboost_a_tree_past_a_bound_is_refused() {
    // ALLOC: test data.
    let many = vec![STUMP; MAX_TREES + 1].join(",");
    let e = parse("model/xgboost+json", &stump(&many), 1).expect_err("refused");
    assert!(e.reason().contains("4096"), "{e}");
    // a chain one level deeper than allowed
    let n = MAX_DEPTH + 2;
    let ids = |f: &dyn Fn(usize) -> String| (0..2 * n + 1).map(f).collect::<Vec<_>>().join(",");
    let left = ids(&|i| {
        if i % 2 == 0 && i < 2 * n {
            // ALLOC: test data.
            (i + 2).to_string()
        } else {
            "-1".into()
        }
    });
    let right = ids(&|i| {
        if i % 2 == 0 && i < 2 * n {
            // ALLOC: test data.
            (i + 1).to_string()
        } else {
            "-1".into()
        }
    });
    let zero = ids(&|_| "0".into());
    // ALLOC: test data.
    let deep = format!(
        r#"{{"left_children":[{left}],"right_children":[{right}],"split_indices":[{zero}],"split_conditions":[{zero}],"default_left":[{zero}]}}"#
    );
    let e = parse("model/xgboost+json", &stump(&deep), 1).expect_err("refused");
    assert!(e.reason().contains("deeper"), "{e}");
    // a tree whose child points back at its root never ends and is refused
    let cycle = r#"{"left_children":[0,-1],"right_children":[1,-1],"split_indices":[0,0],"split_conditions":[0.5,1.0],"default_left":[0,0]}"#;
    assert!(parse("model/xgboost+json", &stump(cycle), 1).is_err());
}

#[test]
fn a_definition_over_the_size_bound_is_refused() {
    let big = " ".repeat(super::MAX_DEFINITION_BYTES + 1);
    assert!(parse("model/linear", &big, 1).is_err());
    let e = Evaluator::parse("model/linear", "{}", &names(super::MAX_FEATURES + 1))
        .expect_err("refused");
    assert!(e.reason().contains("10000"), "{e}");
}

const RANKLIB: &str = "## LambdaMART\n## No. of trees = 2\n## Learning rate = 0.1\n\n<ensemble>\n\
 <tree id=\"1\" weight=\"0.5\">\n  <split>\n   <feature> 1 </feature>\n   <threshold> 0.5 </threshold>\n\
   <split pos=\"left\">\n    <output> -1.0 </output>\n   </split>\n\
   <split pos=\"right\">\n    <feature> 2 </feature>\n    <threshold> 3.0 </threshold>\n\
    <split pos=\"left\">\n     <output> 1.0 </output>\n    </split>\n\
    <split pos=\"right\">\n     <output> 4.0 </output>\n    </split>\n   </split>\n  </split>\n </tree>\n\
 <tree id=\"2\" weight=\"2.0\">\n  <split>\n   <feature> 2 </feature>\n   <threshold> 1.0 </threshold>\n\
   <split pos=\"left\">\n    <output> 0.25 </output>\n   </split>\n\
   <split pos=\"right\">\n    <output> 0.75 </output>\n   </split>\n  </split>\n </tree>\n</ensemble>\n";

#[test]
fn a_ranklib_lambdamart_model_sums_its_weighted_trees() {
    let m = parse("model/ranklib", RANKLIB, 2).expect("parses");
    // tree 1 (0.5): f1 <= 0.5 -> -1 ; tree 2 (2.0): f2 <= 1.0 -> 0.25
    assert!(close(m.score(&[0.5, 1.0]), 0.5 * -1.0 + 2.0 * 0.25));
    // f1 > 0.5, f2 = 3.0 <= 3.0 -> 1.0 ; f2 > 1.0 -> 0.75
    assert!(close(m.score(&[0.6, 3.0]), 0.5 * 1.0 + 2.0 * 0.75));
    // f2 > 3.0 -> 4.0
    assert!(close(m.score(&[0.6, 3.5]), 0.5 * 4.0 + 2.0 * 0.75));
    // a missing feature counts 0
    assert!(close(
        m.score(&[f32::NAN, f32::NAN]),
        0.5 * -1.0 + 2.0 * 0.25
    ));
}

#[test]
fn a_ranklib_random_forest_averages_its_bags() {
    let bag = |out: f32| {
        // ALLOC: test data.
        format!(
            "<ensemble><tree id=\"1\" weight=\"1.0\"><split><output> {out} </output></split></tree></ensemble>"
        )
    };
    // ALLOC: test data.
    let forest = format!(
        "## Random Forests\n## No. of bags = 2\n{}{}",
        bag(2.0),
        bag(4.0)
    );
    let m = parse("model/ranklib", &forest, 1).expect("parses");
    assert!(close(m.score(&[0.0]), 3.0));
}

#[test]
fn ranklib_other_rankers_and_bad_models_are_refused() {
    let e = parse(
        "model/ranklib",
        &RANKLIB.replace("LambdaMART", "AdaRank"),
        2,
    )
    .expect_err("refused");
    assert!(e.reason().contains("AdaRank"), "{e}");
    // a feature beyond the set, and feature 0 (RankLib counts from 1)
    assert!(parse("model/ranklib", RANKLIB, 1).is_err());
    assert!(
        parse(
            "model/ranklib",
            &RANKLIB.replace("<feature> 1 </feature>", "<feature> 0 </feature>"),
            2
        )
        .is_err()
    );
    assert!(parse("model/ranklib", "## LambdaMART\n<ensemble>", 2).is_err());
    assert!(parse("model/ranklib", "## LambdaMART\n<ensemble></ensemble>", 2).is_err());
    assert!(
        parse(
            "model/ranklib",
            &RANKLIB.replace("</ensemble>", "</ensemble><ensemble></ensemble>"),
            2
        )
        .is_err()
    );
}

#[test]
fn a_ranklib_tree_past_a_bound_is_refused() {
    let tree = "<tree id=\"1\" weight=\"1\"><split><output>1</output></split></tree>";
    // ALLOC: test data.
    let many = format!("<ensemble>{}</ensemble>", tree.repeat(MAX_TREES + 1));
    assert!(parse("model/ranklib", &many, 1).is_err());
    let mut nested = String::from("<split>");
    for _ in 0..=MAX_DEPTH {
        nested.push_str("<feature>1</feature><threshold>1</threshold><split pos=\"left\">");
    }
    nested.push_str("<output>1</output>");
    // ALLOC: test data.
    let deep = format!("<ensemble><tree weight=\"1\">{nested}</tree></ensemble>");
    let e = parse("model/ranklib", &deep, 1).expect_err("refused");
    assert!(e.reason().contains("deeper"), "{e}");
}

#[test]
fn ranklib_xml_comments_declarations_and_odd_shapes_are_read_or_refused() {
    // ALLOC: test data.
    let leaf = "<tree><split><output>2</output></split></tree>";
    let ok = format!("<?xml version=\"1.0\"?><!-- a note --><ensemble>{leaf}</ensemble>");
    let m = parse("model/ranklib", &ok, 1).expect("parses");
    // a tree with no weight counts 1
    assert!(close(m.score(&[0.0]), 2.0));
    for bad in [
        "<!-- never closed",
        "<?xml never closed",
        "<ensemble",
        "<ensemble/>",
        "<ensemble><tree weight=\"x\"><split><output>1</output></split></tree></ensemble>",
        "<ensemble><tree><split><output>x</output></split></tree></ensemble>",
        "<ensemble><tree><split><output>1</output></split></ensemble>",
        "<ensemble><tree>oops</tree></ensemble>",
        "<ensemble><oops/></ensemble>",
        "<tree/>",
        "<ensemble><tree><split><feature>1</feature><threshold>1</threshold><split pos=\"left\"><output>1</output></split><split pos=\"left\"><output>1</output></split></split></tree></ensemble>",
        "<ensemble><tree><split><feature>1</feature><threshold>1</threshold><split><output>1</output></split></split></tree></ensemble>",
        "<ensemble><tree><split><feature>1</feature><split pos=\"left\"><output>1</output></split></split></tree></ensemble>",
        "<ensemble><tree><split><feature>1.5</feature><threshold>1</threshold></split></tree></ensemble>",
        "<ensemble><tree><split><oops>1</oops></split></tree></ensemble>",
    ] {
        assert!(parse("model/ranklib", bad, 1).is_err(), "{bad}");
    }
}

#[test]
fn xgboost_base_score_forms_and_bad_values_are_handled() {
    let m = stump(STUMP);
    // a numeric base score, and a logistic one outside (0, 1)
    let numeric = m.replace("\"5E-1\"", "0.25");
    assert!(close(
        parse("model/xgboost+json", &numeric, 1)
            .expect("parses")
            .score(&[0.0]),
        0.25 - 1.0
    ));
    let logistic = m.replace("reg:squarederror", "reg:logistic");
    assert!(close(
        parse("model/xgboost+json", &logistic, 1)
            .expect("parses")
            .score(&[0.0]),
        1.0 / (1.0 + 1.0_f32.exp())
    ));
    assert!(
        parse(
            "model/xgboost+json",
            &logistic.replace("\"5E-1\"", "\"1.5\""),
            1
        )
        .is_err()
    );
    assert!(parse("model/xgboost+json", &m.replace("\"5E-1\"", "\"oops\""), 1).is_err());
    assert!(parse("model/xgboost+json", &m.replace("\"5E-1\"", "true"), 1).is_err());
    assert!(
        parse(
            "model/xgboost+json",
            &m.replace(",\"objective\":{\"name\":\"reg:squarederror\"}", ""),
            1
        )
        .is_err()
    );
    // arrays that differ in length, an empty tree, a child outside the tree, a bad array
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace("[0.5,-1.0,2.0]", "[0.5]")),
            1
        )
        .is_err()
    );
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace("[1,-1,-1]", "[]")),
            1
        )
        .is_err()
    );
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace(
                "\"right_children\":[2,-1,-1]",
                "\"right_children\":[9,-1,-1]"
            )),
            1
        )
        .is_err()
    );
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace("[1,-1,-1]", "\"x\"")),
            1
        )
        .is_err()
    );
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace("[1,-1,-1]", "[\"x\"]")),
            1
        )
        .is_err()
    );
    assert!(
        parse(
            "model/xgboost+json",
            &stump(&STUMP.replace("[0.5,-1.0,2.0]", "[true,0,0]")),
            1
        )
        .is_ok()
    );
    assert!(parse("model/xgboost+json", &stump("1"), 1).is_err());
    assert!(
        parse(
            "model/xgboost+json",
            &m.replace("\"trees\":[", "\"trees\":{\"a\":[")
                .replace("]}}}}", "]}}}}}"),
            1
        )
        .is_err()
    );
    // fields of no interest are skipped
    assert!(
        parse(
            "model/xgboost+json",
            &m.replace(
                "\"learner\":{",
                "\"version\":[2,1],\"learner\":{\"x\":{\"y\":1},"
            ),
            1
        )
        .is_ok()
    );
}
