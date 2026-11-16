# Request budgets: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-testkit`, `cargo test -p oss-blob --features emulator --test rounds`, `cargo test -p oss-meta --test descriptors budgets`, `cargo test -p oss-node --test it budgets`
- each gate in record mode (`OSS_REQUEST_BUDGETS_RECORD=1`), to fill in the table's numbers
- `scripts/selftest-checks.py`, which plants each violation of `scripts/check-budgets.py`
- the whole gate, `scripts/gate.sh`

An independent review found a blocked target attributed to the Java with no Java source, six Java rows dropped (among them the idle-hour rows and the Java's account of why an idle held index makes requests), a tree check that a row could pass with no gate running, the emulator's one-request register swap stated as S3's, the index delete measured with heads and pins stubbed but not saying so, sequential round-trip rows left as ceilings, and a depth test that could fail on a loaded machine. Each is fixed: the rows are restored with the Java's numbers and text, the check follows the test target, the attributes and the function body, and the depth test starts its calls at a barrier.

- AC-1: `the_table_parses_and_keeps_a_blocked_row`, `a_malformed_row_is_an_error_not_a_skip`
- AC-2: `a_regression_fails_and_names_the_budget`, `an_exact_saving_fails_until_the_table_records_it`, `check_refuses_an_unknown_or_blocked_objective`, `record_mode_checks_nothing`
- AC-3: `calls_sent_together_share_a_round`, `a_call_after_an_answer_is_the_next_round`, `the_log_names_each_call_and_its_key`
- AC-4: `scripts/selftest-checks.py` (ten planted violations of `check-budgets.py`: a missing test file, a missing test function, an ignored test, a function that is no test, a test that never names its key, a key outside the test's body, a test module no target declares, a blocked row with no blocker, a third status, an objective twice)
- AC-5: `a_cold_search_then_a_warm_one`, `descriptor_operations_cost_what_the_table_says`, `an_index_create_and_delete_cost_what_the_table_says`, `an_alias_update_costs_what_the_table_says`, `a_cluster_settings_update_costs_what_the_table_says`

`cargo mutants` over the diff in `oss-testkit` and `oss-blob` with all features: 111 mutants, 1 missed (a `> 0` guard on a zero sleep, an equivalent mutant), and the guard was removed; after the review's fixes, 108 mutants, 0 missed.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| a call's round counted from the deepest call started rather than the deepest answered | `calls_sent_together_share_a_round` |
| an exact row's saving passing | `an_exact_saving_fails_until_the_table_records_it` |
| an objective twice accepted | `a_malformed_row_is_an_error_not_a_skip` |
| a blocked row checked as if enforced | `check_refuses_an_unknown_or_blocked_objective` |
| the table's descriptor update budget lowered below what it costs | `descriptor_operations_cost_what_the_table_says` |
| the table's cold search budget raised above what it costs | `a_cold_search_then_a_warm_one` |
