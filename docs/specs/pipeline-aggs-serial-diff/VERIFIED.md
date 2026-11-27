# Verified: serial_diff pipeline aggregation

Recorded on 2026-11-27 against the 3.10.0 jars from Maven Central: `PipelineSerial.java` wrote `java-serial-3.10.0.txt` (56 lines: the 48 histogram cases, the empty-bucket cases and the builder refusals), `PipelineSerialParse.java` wrote `java-serial-parse-3.10.0.txt` (bodies and validations), and `PipelineWireRecordings.java` wrote the `serial_diff` lines of `java-pipeline-wire-3.10.0.txt`. The Rust output matched at the first run for every computed line; the first parse run failed only on a test expectation (a `buckets_path` of `[1]` is read, then fails validation for want of an aggregation of that name, as Java's would).

- AC-1: `serial_diff_gives_the_bytes_java_wrote` (`crates/reader/src/aggs/pipeline/serial_tests.rs`)
- AC-2: `a_bucket_with_no_document_is_a_gap_for_serial_diff`
- AC-3: `a_typed_serial_diff_result_is_a_simple_value`
- AC-4: `a_body_is_read_as_java_reads_it`
- AC-5: `serial_diff_is_validated_as_java_validates_it`
- AC-6: `a_serial_diff_works_on_the_buckets_that_hold_it` (`crates/node/tests/it/search_pipelines.rs`)
- AC-7: `a_serial_diff_reads_from_java_bytes` (`crates/transport/tests/search_shard_pipeline.rs`)

Not run locally: the full gate, coverage and mutation (left to CI).
