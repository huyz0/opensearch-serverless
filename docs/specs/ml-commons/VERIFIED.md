# Verified

Slice 1a (model groups and their name claims, `crates/node/src/ml_route/`) is built and on by default (`serverless.ml.enabled`, `ml_commons_is_on_by_default_and_turned_off_by_its_variable`). The other criteria belong to later slices.

- AC-1: `a_model_group_is_registered_read_updated_searched_and_deleted`, `a_name_in_use_is_refused_naming_its_holder_and_is_free_after_a_delete_or_rename` (`crates/node/tests/server/ml_routes.rs`, over the emulator). Shapes follow the reference plugin's Java and stay provisional: no stock run. Slice 1a covers model groups; `shapes.md` is not written yet. The tests were written after the code; seen red with planted faults (a claim never released, a duplicate name not refused): `a_name_in_use_...` and `a_set_holds_at_most_...` failed, then the faults were removed.
- AC-2: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-3: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-4: `a_model_group_costs_one_get_and_three_puts_and_nothing_lists`, `a_search_reads_a_page_of_documents_not_the_set`: a register is 1 GET and 3 PUTs, a read 2 GETs, an exact name search 4 GETs, a page 1 + 2 GETs a hit, no LIST (emulator counts). Model group only; connectors and models come with slices 1b and 2.
- AC-5: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-6: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-7: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-8: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-9: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-10: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-11: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-12: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-13: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-14: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-15: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-16: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-17: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-18: `a_malformed_register_is_400_and_never_a_panic` (register only; a body is bounded by the document bound). Other routes come with their slices.
- AC-19: NOT-RUN: a later slice (connectors 1b, remote models 2, local models 3, stats and tools 4, agents and memory 5)
- AC-20: `a_set_holds_at_most_a_thousand_groups_and_a_refused_register_keeps_no_claim` (the 1,000-document bound, 429, the claim released); `a_search_reads_a_page_of_documents_not_the_set` (a size past 100 is 400, a page reads only its hits). `a_name_word_search_over_a_large_set_is_refused` (past 200 groups).
