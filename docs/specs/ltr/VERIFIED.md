# Verified

Slices 1 (evaluators, `crates/reader/src/ltr/`) and 2 (store and routes, `crates/node/src/ltr_route/`) are built; slices 3 to 5 are not. `_clearcache` and `_stats` come with slice 5.

- AC-1: `a_linear_model_scores_the_dot_product_of_its_weights`, `a_linear_model_naming_a_feature_the_set_lacks_is_refused` (the evaluator refuses; the 400 on a put comes with slice 2). Seen red with a stub scoring 0.
- AC-2: `xgboost_squared_error_scores_as_xgboost_does`, `xgboost_reg_logistic_scores_as_xgboost_does`, `xgboost_binary_logistic_scores_as_xgboost_does`, `xgboost_a_missing_value_follows_its_default_direction`: three models XGBoost 2.1.4 trained and saved (`fixtures/ltr/gen_xgboost.py`) and the scores its predict gave on eight vectors each, two with missing values, equal within 1e-5 relative. Seen red with a stub scoring 0.
- AC-3: `a_ranklib_lambdamart_model_sums_its_weighted_trees`, `a_ranklib_random_forest_averages_its_bags`, `ranklib_other_rankers_and_bad_models_are_refused`. Provisional: no RankLib build was available, so the expected scores are worked by hand from RankLib's documented semantics (`<=` goes left, tree weights, bags averaged), not recorded from the tool; a recorded run replaces them when a RankLib jar is at hand. MART and LambdaMART share one file format and one test.
- AC-4: `a_definition_over_the_size_bound_is_refused`, `xgboost_a_tree_past_a_bound_is_refused`, `a_ranklib_tree_past_a_bound_is_refused` (size, trees, depth, features, a cycle), `a_malformed_definition_is_refused_with_its_position`, `ranklib_xml_comments_declarations_and_odd_shapes_are_read_or_refused`, `xgboost_base_score_forms_and_bad_values_are_handled`: a refusal, never a panic.
- AC-5: `a_store_a_feature_set_and_a_model_are_created_read_replaced_listed_and_deleted`, `a_named_store_holds_its_own_objects`, `an_invalid_feature_set_or_model_is_a_400` (node, `tests/server/ltr_routes.rs`), over the emulator: shapes follow `shapes.md` and stay provisional (no stock run). Seen red with the routes unregistered (5 of 5 failed). Differences: _version is the set's change count and is absent from a read and a delete; the 400 reasons are this node's own wording.
- AC-6: `a_put_costs_one_get_and_two_puts_a_read_two_gets_and_nothing_lists`: a put 1 GET and 2 PUT, a read 2 GET, no LIST (emulator counts). A list is the marker plus one GET for each object of its page.
- AC-7: `addfeatures_extends_a_set_and_createmodel_scores_its_features`: addfeatures makes or extends a set and refuses a duplicate name, createmodel stores a model with its set embedded and checks the definition against the set; a version bump is the set's change count rising (`a_created_set_is_missing_until_created_and_a_model_may_exceed_the_mebibyte` in `oss-meta`).
- AC-8: NOT-RUN: slice 3 is not built yet
- AC-9: NOT-RUN: slice 3 is not built yet
- AC-10: NOT-RUN: slice 3 is not built yet
- AC-11: NOT-RUN: slice 4 is not built yet
- AC-12: NOT-RUN: slice 3 is not built yet
- AC-13: NOT-RUN: slices 2 and 3 are not built yet
