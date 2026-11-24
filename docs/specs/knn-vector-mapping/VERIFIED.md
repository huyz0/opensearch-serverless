# Verified

Recorded from the stock OpenSearch 3.10.0 release tarball (build `b9f09249`, SHA-512 matched the published sum) with its bundled k-NN plugin, run on one node here; the fixtures say so in `recorded_from`.

- AC-1: `mapping_steps_replay` in crates/doc/tests/mapping_fixtures.rs, over `fixtures/compat/mapping/knn-mapping.json`, `knn-invalid.json`, `knn-merge.json` and `knn-noset.json`. Before the type existed the same fixtures failed in it (the field was refused as unsupported where the plugin accepts it).
- AC-2: `a_caller_that_does_not_ask_for_knn_vector_is_told_it_is_unsupported` in crates/doc/tests/knn_mapping.rs
- AC-3: `the_shapes_this_writer_does_not_index_are_unsupported_not_invalid` in crates/doc/tests/knn_mapping.rs
- AC-4: `a_field_with_a_method_on_an_index_without_knn_is_refused` in crates/doc/tests/knn_mapping.rs; also `knn-noset.json` in AC-1
- AC-5: `the_order_of_the_method_parameters_is_not_part_of_the_mapping`, `the_method_is_written_with_its_resolved_space_and_reads_back` in crates/doc/tests/knn_mapping.rs
- AC-6: `a_method_that_can_be_neither_unsupported_nor_valid_is_refused_in_the_plugins_order` in crates/doc/tests/knn_mapping.rs

Found while recording: the plugin stores a vector field's `_source` without the vector and rebuilds it (as floats, `[1,2,3]` comes back `[1.0,2.0,3.0]`) on every fetch. The document step has to do the same to read a Java node's data and be read by it.

Not covered: the engines the plugin resolves to when a method names none (Faiss; its native libraries are not in the release tarball), so those mappings stay unsupported; the order of errors when one mapping has several faults (each fixture has one).
