# Verified: restore-default-flags

- AC-1: `a_flag_at_the_value_this_node_applies_is_accepted`.
- AC-2: `what_the_shell_refuses_is_refused_with_its_reason`.
- AC-3: `a_flag_that_is_not_a_boolean_is_400`.
- AC-4: `a_refused_key_without_a_default_stays_refused`.
- AC-5: `flags_at_their_defaults_cost_nothing`.

AC-1 and AC-3 were seen red before the code existed (two of the three coordinator tests failed); AC-4 passed from the start because it pins existing behaviour; AC-5 passed on its first run.
