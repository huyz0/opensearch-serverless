# Reader: `global` aggregation: verified

Run on 2026-11-27 (Linux cloud container). No stock OpenSearch recording backs these steps; expected values are read from the 3.10.0 Java. The tests were written with the code and passed at the first run, so none was seen failing first.

- AC-1: `a_global_body_is_empty_and_means_match_all`
- AC-2: `a_global_counts_every_document_whatever_the_query_and_keeps_request_order`
- AC-3: `a_global_is_top_level_only_and_holds_no_top_hits`, `a_top_hits_under_a_global_is_refused`
