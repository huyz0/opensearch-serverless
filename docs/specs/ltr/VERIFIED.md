# Verified

Slice 1 (evaluators, `crates/reader/src/ltr/`) is built; slices 2 to 5 are not.

- AC-1: `a_linear_model_scores_the_dot_product_of_its_weights`, `a_linear_model_naming_a_feature_the_set_lacks_is_refused` (the evaluator refuses; the 400 on a put comes with slice 2). Seen red with a stub scoring 0.
- AC-2: `xgboost_squared_error_scores_as_xgboost_does`, `xgboost_reg_logistic_scores_as_xgboost_does`, `xgboost_binary_logistic_scores_as_xgboost_does`, `xgboost_a_missing_value_follows_its_default_direction`: three models XGBoost 2.1.4 trained and saved (`fixtures/ltr/gen_xgboost.py`) and the scores its predict gave on eight vectors each, two with missing values, equal within 1e-5 relative. Seen red with a stub scoring 0.
- AC-3: `a_ranklib_lambdamart_model_sums_its_weighted_trees`, `a_ranklib_random_forest_averages_its_bags`, `ranklib_other_rankers_and_bad_models_are_refused`. Provisional: no RankLib build was available, so the expected scores are worked by hand from RankLib's documented semantics (`<=` goes left, tree weights, bags averaged), not recorded from the tool; a recorded run replaces them when a RankLib jar is at hand. MART and LambdaMART share one file format and one test.
- AC-4: `a_definition_over_the_size_bound_is_refused`, `xgboost_a_tree_past_a_bound_is_refused`, `a_ranklib_tree_past_a_bound_is_refused` (size, trees, depth, features, a cycle), `a_malformed_definition_is_refused_with_its_position`, `ranklib_xml_comments_declarations_and_odd_shapes_are_read_or_refused`, `xgboost_base_score_forms_and_bad_values_are_handled`: a refusal, never a panic.
- AC-5: NOT-RUN: slice 2 is not built yet
- AC-6: NOT-RUN: slice 2 is not built yet
- AC-7: NOT-RUN: slice 2 is not built yet
- AC-8: NOT-RUN: slice 3 is not built yet
- AC-9: NOT-RUN: slice 3 is not built yet
- AC-10: NOT-RUN: slice 3 is not built yet
- AC-11: NOT-RUN: slice 4 is not built yet
- AC-12: NOT-RUN: slice 3 is not built yet
- AC-13: NOT-RUN: slices 2 and 3 are not built yet
