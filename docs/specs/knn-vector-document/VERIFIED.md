# Verified

Replayed against the fixtures recorded from the stock OpenSearch 3.10.0 release with its bundled k-NN plugin (`fixtures/compat/knn/`, `recorded_from` in each).

- AC-1: `mapping_steps_replay` in crates/doc/tests/mapping_fixtures.rs, over `fixtures/compat/knn/knn-documents.json` and `knn-cosine.json`. Before the walker read vectors, the same replay failed on the first accepted document (`unsupported_mapping: knn_vector values`).
- AC-2: `a_vector_is_read_beside_other_fields_and_inside_an_object` in crates/doc/tests/knn_document.rs
- AC-3: `the_shapes_left_to_a_java_writer_are_unsupported_not_invalid` in crates/doc/tests/knn_document.rs
- AC-4: `a_component_is_the_float_java_reads` in crates/doc/tests/knn_document.rs
- AC-5: NOT-RUN: the vector is one `Vec<f32>` of at most the mapping's dimension (16,000); no allocation bound is measured here.

Not covered: base64 vectors (the plugin accepts valid base64 for binary vectors; not recorded for float, so left unsupported), `data_type` other than `float`, and what the plugin does with a vector longer than the dimension limit (the mapping refuses the dimension first).
