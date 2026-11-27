# LTR fixtures

`xgboost-<objective>-model.json` is a model XGBoost 2.1.4 saved with `save_model`;
`xgboost-<objective>-scores.txt` holds the scores that XGBoost's `predict` gave on fixed
vectors, one `v1 v2 v3 v4 -> score` a line, `nan` for a missing value.
`gen_xgboost.py` regenerates them (`pip install xgboost==2.1.4 numpy`). The RankLib and
linear tests in `crates/reader/src/ltr/tests.rs` are hand-built: no RankLib build was
available when they were written, so their expected scores follow RankLib's documented
semantics (`<=` goes left, tree weights, Random Forests averaged), not a tool run.
