# Extension point: analyzer

## Serves

R15.3's first point, decided in [ADR 0053](../../adr/0053-the-analyzer-extension-point.md) on the runtime of [ADR 0052](../../adr/0052-extension-modules-on-a-wasm-abi.md). Rules a point follows: [add-an-extension-point](../../how-to/add-an-extension-point.md).

## Change

An index can pin a WASM module as a named analyzer at creation, in `index.analysis.analyzer.<name>.{type,module,version}`; mappings name it as stock does. The module analyses at index time, replay, query time, highlight and `_analyze`. Delivered in the five slices of the ADR; **this change is the ADR and this spec only**, no code. Not here: pins on an existing index, token graphs, module imports, a result cache, a lease capability, multi-term normalisation.

## Acceptance criteria

- AC-1: The point's codec round-trips blocks of 1 to 64 texts and rejects, with a typed failure naming the field, a count of 0 or over 64, a text over the 1 MiB block ceiling, an output over 4 MiB or 100,000 tokens, and trailing bytes.
- AC-2: The host refuses a module's output (502 `extension_malformed`) for an empty or non-UTF-8 term, a term over 32,766 bytes, `start > end`, an end past the text, an offset inside a character, a decreasing start, a first increment of 0, and a position sum past Lucene's maximum and counts that do not fit; each is a separate test.
- AC-3: A module with any import, or one that does not declare `analyzer` version 1, is refused at pin time and never called.
- AC-4: A create with a pin is refused (400 naming the key) when the module is not in the store, its descriptor does not declare the pinned point and version, it is refused by the ABI, it traps on the fixed probe text, or the name is not `ext_[a-z0-9_]{1,60}` (so `standard`, `default`, `keyword` and every built-in name are refused), the `version` is not exactly the point's, there are over 4 pins or the pin settings pass 16 KiB; a mapping naming an unpinned custom analyzer is still refused as today.
- AC-5: The pin's settings in the descriptor carry the resolved `fuel`, `memory_bytes` and `deadline_ms`; a later change of the host defaults changes neither a write's nor a replay's recorded limits (the replay factor is the one operator-set input) (test: change the defaults, replay, same result); rewriting the settings keeps every other byte of them (round-trip test).
- AC-6: `index.analysis.*` in an update of settings or mapping answers the Java's final-setting error; a mapping update may add a field using an existing pin.
- AC-7: A pin through a template (stored and resolved), rollover or restore is validated as a create is or refused with a 400; `index.analysis` in any spelling needs `all_access` in the handler and a client `limits.*` key is refused; setting the cluster setting back to `false` leaves existing pins serving.
- AC-8: The overlay build of a pinned tail uses the module (differential module, counter), bypasses the cap and gate, and on failure answers 503 `extension_unavailable` rather than a result without the tail.
- AC-9: A creating role other than `all_access` is refused for a pinned create, and `serverless.extensions.pins.allowed=false` (the default) refuses any pin; an index with no pin is created and served identically with the setting on or off and the runtime built or not (the existing analysis tests unchanged).
- AC-10: A field analysed by a module yields the terms, positions, offsets, norm length and gap handling the built-in `standard` gives for a module that reimplements it (a golden module), including a multi-valued field across the gaps, across a split at 64 values, and offsets after astral characters (UTF-8 byte to UTF-16 unit). Because a fallback to the built-in would pass that, every other AC about use of the module uses a **differential module** whose output matches no built-in (upper-cased terms and bigrams) and a call counter, and asserts both the output and that the counter moved.
- AC-11: A text over 1 MiB and a text over 100,000 tokens are refused 400; a field of over 64 values or 1 MiB is split between texts without changing positions or offsets; a cumulative cap of ten deadlines refuses an item with many calls. An item whose analysis fails (limit, malformed output, a full gate) is refused with its status and **no record of it reaches the node log**, asserted by reading the log; the other items of the bulk are unaffected.
- AC-12: Replay under a full gate and tenant cap still reproduces the same terms (replay bypasses both), and a reservation refused for 60 s fails the open as retryable `extension_unavailable`, skipping nothing. Replay after a crash and after a takeover reproduces the same terms, byte for byte, as the first run for a log of pinned writes; a replay whose module now exceeds the recorded fuel fails the shard closed with 503 `extension_replay_failed` naming the seq_no, and the write is neither skipped nor indexed without terms; raising `OSS_EXTENSIONS_REPLAY_FUEL_FACTOR` and reopening makes that shard replay and open with no write lost; replay's deadline is ten times the recorded one.
- AC-13 (every site, with the differential module): `match`, `match_phrase`, `multi_match`, `query_string`, `simple_query_string`, and the `search`, `count`, `msearch`, frozen and shard paths, `_validate/query`, the highlighter and `_analyze` each analyse a pinned field with the module; the other analysing queries named in the ADR, and prefix, wildcard, regexp and fuzzy on a pinned field, are refused as unsupported; the tail overlay indexes a pinned pushed document with the module and the fast plan declines the field. A tree check for the analyzer sites fails on a planted `Analyzer::standard(` outside the resolver and is in `docs/mechanical-gates.md`.
- AC-14: A node that cannot run a pin (no runtime, percent 0, module absent, hash or ABI mismatch) refuses the open with 503 `extension_unavailable` and never serves the index with the built-in analyzer; a pool too small for the compile answers 429; a refused open is cached negatively for 30 s per hash.
- AC-15: Opening a pinned index adds one GET per distinct module per node in the open's existing round (sequential depth unchanged), none per shard, query, document or highlight; a node with the module loaded adds none; LIST 0. A budget row holds it.
- AC-16: Hostile modules at the point (spin, memory growth, trap, oversized output, malformed output, concurrent callers over the gate) fail their calls with the typed status and leave the node serving within its memory budget; the peak never passes the reservation of ADR 0052.
- AC-17: Per-tenant counters (`calls`, `fuel`, `input_bytes`, `output_bytes`) are exact over a counted run and appear on `GET /_serverless/usage` when it is on.
- AC-18: An open pinned index's module is resident: under pool pressure from other modules it is not evicted, a call makes no GET (counted) and an open that cannot be charged answers 429; a takeover by a node without the module makes no fencing write, so the head stays claimable by another node. A tenant's third concurrent call is refused 429 `extension_busy` (the cap of 2 of 8); counters are bounded by the ledger's `max_indices` and `_other`; takeover by a node without the module leaves the shard unserved under back-off, with the metric and a bounded retry cost; a pinned module is refetched at most once per 30 s per hash.
- AC-19: The Java shell refuses to open a descriptor with a pin (`fixtures/extensions/CheckPin.java`, run by hand, and it also feeds the cluster-config key to a Java node; NOT-RUN until recorded in VERIFIED.md); a tree check fails when a shipped configuration sets the cluster setting to `true`; it stays `false` until that line, the hostile soak, the replay equality and a recorded write p99 and per-query call cost exist.

## Test plan

Each AC names its test as the slice that delivers it lands; a test is seen red before the code. AC-8 and AC-10 use a golden module built from the built-in analyzer's rules so a divergence is a failing diff. AC-9 and AC-10 use the node log's reader, not the engine's state. AC-16 is a fixture run by hand, as the Java mixed-fleet proofs are.

## S3 request budget

Open of a pinned index: one GET per distinct module per node, a sibling of the commit GETs. Create with a pin: that GET plus the descriptor's writes as today. Calls, documents, queries, highlights: 0. LIST 0. No row is added before the code, because a row must be spent by a test.

## Switch

Cargo feature `extensions` with `OSS_EXTENSIONS_PERCENT` (above 0), and the dynamic cluster setting `serverless.extensions.pins.allowed`, default `false`. Flip order and the evidence it waits for: ADR 0053 section 7.

## Risks

- The Java refusal is assumed from its analyzer registry until AC-16 runs; the setting stays off until it does.
- Fuel can change with a wasmtime version: held by the golden vectors and the fail-closed replay.
- A pin cannot change, so a bad module means a reindex; creation probes the module to catch the always-failing case.
- A mis-deployed fleet shows as unserved shards, not wrong results; no lease capability in v1.
