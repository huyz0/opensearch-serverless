# Reader: vector files warm at the first vector search: verified

Run on 2026-11-24 (Linux cloud container, `userfaultfd` available, emulator and RustFS 1.0.6 on loopback; provisional numbers).

- AC-1: `the_warm_at_open_reads_every_vector_file_at_open` (2 segments, 7.55 MB: the open reads all of it).
- AC-2: `a_term_search_of_a_vector_index_reads_no_vector_file`.
- AC-3: `a_knn_search_reads_the_same_bytes_in_no_more_requests_or_round_trips`. Seen red first, with only the engine's switch off and no warm: the first knn search made many faulted requests at a depth of about 26 on a 5-segment index where the old open and search made 28 at depth 7. With the warm: open 0.73 MB in 11 requests, first knn search 6.82 MB in 2 requests, together 7.55 MB (the old open's bytes) in 13 requests at depth 5 (old 13 at depth 5); second search 0.
- AC-4: `a_knn_search_reads_the_same_bytes_in_no_more_requests_or_round_trips` compares the knn hits and scores bit for bit between the modes, and the term search's hits too.
- AC-5: `boot::config::tests::the_vector_warm_is_at_the_first_knn_search_unless_asked_to_be_at_open`.
- AC-6: `a_refused_warm_leaves_the_search_to_its_own_reads_with_the_same_answer` in crates/reader/tests/it/vector_warm.rs: the store refuses the first three ranged reads of the first knn search (the meter counts three errors) and the search answers the hits and scores of a clean run, bit for bit. Written after the behaviour, not seen red; the refused reads are the first the search makes, which are the warm's, but the test does not tell a refused warm from a refused fault. NOT-RUN: a refusal of every ranged read (the search is expected to fail then, as any read failure does).
- Scale (`knn_requests`, 100,000 vectors of 128 dimensions, 47 MB, 10 segments), emulator and RustFS gave the same counts:

| mode | step | term search | knn search |
|---|---|---|---|
| at open (old) | open | 47.2 MB, Get=2 GetRange=61 | the same |
| at open (old) | first search | 0 | 0 |
| at first `knn` (new) | open | 2.67 MB, Get=2 GetRange=41 | the same |
| at first `knn` (new) | first search | 0 | 44.56 MB, GetRange=10 |

A term search of the index reads 2.67 MB in 43 requests instead of 47.2 MB in 63; a `knn` search reads the same 47.2 MB in 53 requests instead of 63. - Latency (`LATENCY=s3 knn_requests`: emulator with the Java test kit's S3 profile, GET and range 20 to 30 ms, no bandwidth model, three runs each, same 47 MB index, wall time of a cold open plus first search):

| mode | term search | knn search |
|---|---|---|
| at open (old) | 388 to 401 ms | 403 to 417 ms |
| at first knn (new) | 238 to 268 ms | 304 to 308 ms (open 236 to 239, search 68 to 70) |

The new default is faster for both here, because fewer requests run in fewer rounds; the emulator charges no time per byte, so a real store's transfer time adds the same to a cold knn in both modes and is saved by a term search in the new one.

Not measured: bandwidth on a real network, a host without `userfaultfd`, the node setting end to end.
