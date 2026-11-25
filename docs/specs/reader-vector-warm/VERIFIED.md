# Reader: vector files warm at the first vector search: verified

Run on 2026-11-24 (Linux cloud container, `userfaultfd` available, emulator and RustFS 1.0.6 on loopback; provisional numbers).

- AC-1: `the_warm_at_open_reads_every_vector_file_at_open` (5 segments, 10.7 MB: the open reads all of it).
- AC-2: `a_term_search_of_a_vector_index_reads_no_vector_file`.
- AC-3: `a_knn_search_reads_the_same_bytes_in_no_more_requests_or_round_trips`. Seen red first, with only the engine's switch off and no warm: the first knn search made 24 requests at depth 26 where the old open and search made 28 at depth 7. With the warm: open 1.51 MB in 23 requests, first knn search 9.18 MB in 5 requests, together 10.69 MB (the old open's bytes) in 28 requests at depth 6 (old 7); second search 0.
- AC-4: `a_knn_search_reads_the_same_bytes_in_no_more_requests_or_round_trips` compares the knn hits and scores bit for bit between the modes, and the term search's hits too.
- AC-5: `boot::config::tests::the_vector_warm_is_at_the_first_knn_search_unless_asked_to_be_at_open`.
- AC-6: NOT-RUN: no test makes the store refuse a vector range; read in review, `vector_warm::warm` discards the prefetch's result and a refused page is fetched again by the fault that needs it.
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
