# Verified

- AC-1: `with_no_upstream_a_plugin_path_is_the_ordinary_400` (crates/node/tests/plugin_proxy.rs)
- AC-2: `a_request_reaches_the_upstream_whole_and_its_answer_comes_back`, `every_plugin_prefix_is_proxied_and_a_native_route_is_not`, `head_has_no_body_and_options_is_forwarded`, `a_megabyte_body_streams_through`
- AC-3: `a_prefix_matches_whole_segments_only`, `an_ordinary_route_beats_a_prefix_and_keeps_its_own_405` (crates/rest/tests/server.rs), `every_plugin_prefix_is_proxied_and_a_native_route_is_not`
- AC-4: `a_refused_connect_tries_the_next_upstream_and_all_refused_is_502`
- AC-5: `a_body_over_the_limit_is_413_and_never_reaches_the_upstream`
- AC-6: `an_upstream_that_does_not_answer_is_504`. NOT-RUN: a stall mid-body after the head went out (the per-read timeout path)
- AC-7: NOT-RUN: a 1 GiB body under a peak-memory assertion; `a_megabyte_body_streams_through` covers 1 MiB, and the chunk buffer is 16 KiB by construction
- AC-8: NOT-RUN: nothing to measure; `PluginProxy::new` takes no store, so it cannot make an object-store request
- AC-9: `a_request_reaches_the_upstream_whole_and_its_answer_comes_back`
- AC-10: `a_path_that_could_aim_the_request_elsewhere_is_refused`, `an_upstream_that_is_not_an_origin_is_refused`
- AC-11: `a_chunked_request_body_is_reframed_for_the_upstream`, `a_request_reaches_the_upstream_whole_and_its_answer_comes_back`
- AC-12: `an_ordinary_route_beats_a_prefix_and_keeps_its_own_405`, `head_has_no_body_and_options_is_forwarded`
