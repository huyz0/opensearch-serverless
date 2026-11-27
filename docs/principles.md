# Principles

## Mission

- **Compatible with OpenSearch, scoped to serverless.** Same REST API, query DSL, mappings and responses for everything in scope; a difference is documented, an unknown parameter fails explicitly. *Check:* the compatibility suite ([compatibility.md](compatibility.md)), invariant I-13.
- **Port, don't reinvent.** Port from OpenSearch and the Java serverless shell, keeping each Rust module traceable to its Java source. *Check:* `scripts/check-port-map.py` ([porting.md](porting.md)).
- **Cross-reference for hsearch and pstore.** Take ideas, not code; offer lessons back. lucene-rust is the only one used as code; other crates are the writing thread's choice, pinned, maintained, minimal and licence-compatible ([ADR 0002](adr/0002-runtime-dependencies.md)).

The rules below carry over from the Java serverless shell (huyz0/OpenSearch, branch `feature/serverlessshell`) and from lucene-rust. They bind every change, and this file is their one home: AGENTS.md and the skills point here instead of restating them. Where a rule has a mechanical check, the check is named; where it has none, that is said, and review enforces it.

## Cost: S3 requests are the bill

- **Minimise S3 calls.** Count requests per operation, retries included. Requests scale with nodes and bytes, never with documents, indices or elapsed time per index.
- **LIST is priced like a PUT** (and returns at most 1,000 keys). Prefer a key derived from data already held, then a GET, then a LIST. Every LIST call site carries a `// LIST:` justification. *Check:* `scripts/check-s3-access.py`.
- **No extra PUTs.** Carry information inside uploads that already happen (a commit's metadata rides in the segment or log upload). Never add PUTs to save the same number of LISTs or fewer.
- **Every S3 call is metered.** Only `oss-blob` may depend on an S3 client, and its store records each attempt in a `Meter`. Tests assert request counts ("this write issues exactly one PUT"). *Check:* `scripts/check-s3-access.py`; the count assertions are tests.
- **Idle indices make zero periodic requests.** No timer, poll or backstop per index. At 100M indices, one GET per index every 5 minutes is about $350K a month. *Check:* `an_idle_index_makes_no_requests` (invariant I-4).
- **No data-dependent chain of fetches on a user-facing path.** Fan-out within a round is free; depth is not.

## Write path

- **One write log per node, never per shard.** Writes from every shard a node holds share its log, so request count scales with nodes.
- **One immutable blob per segment.** Files are packed into one object with an offset header; a blob is never rewritten, so its name is its version and caches never go stale.
- **Fencing by conditional create, not locks.** A paused or replaced writer cannot overwrite its successor. No lease or leader election for correctness.

## Memory: bounded, streaming

Decision and detail: [ADR 0004](adr/0004-bounded-memory-and-streaming.md).

- **Every node has a memory budget** split into pools; a component reserves before it allocates. Over budget means 429 or backpressure, never out-of-memory.
- **Fixed-size pooled buffers and per-request arenas.** Steady-state request paths do not call the global allocator.
- **Stream end to end:** REST bodies in and out, node-to-node transport, object-store reads and writes. Nothing reads a whole body, blob or result set into memory; bulk bodies go through a streaming parser.
- **Bounded channels and queues only.** A full queue pushes back.
- **No unbounded `Vec`, `String` or map growth on a request path.** Every allocation there states its bound.

*Check:* `clippy.toml` disallowed methods (whole-buffer reads, unbounded channels); `scripts/check-alloc.py` (every allocation in the request-path crates carries `// ALLOC:`); peak-memory tests under load (invariant I-17).

## Switches and defaults

- **Build behind switches.** A new behaviour lands behind a setting or a Cargo feature.
- **On by default only when measured with no downside:** write p99 within 5%, no extra S3 requests, no search regression, crash and correctness tests passing.
- **No downside, no switch.** A change that wins everywhere ships without one. Remove a switch once its default is proven.
- Existing tests keep pinning the old behaviour while the switch exists; new tests cover the new one; both run.

*Check:* none mechanical; review.

## Code shape

- **Small modules:** under 500 lines ideally, 1,000 at most. *Check:* `scripts/check-module-size.py`.
- **One concern per module.** No `util`, `misc` or `common` dumping grounds.
- **Layered crates, no cycles, siblings independent.** *Check:* `scripts/check-layering.py`.
- **Private by default.** `pub` only for what another crate needs; every public item has a doc comment. *Check:* `missing_docs = "deny"` in the workspace lints.
- **`unsafe` only when guaranteed safe:** any crate, only the patterns in [unsafe.md](unsafe.md), each block private behind a safe API with a `// SAFETY:` comment; safe first, `unsafe` only where a profile shows it pays. *Check:* `unsafe_code = "deny"` plus clippy's unsafe lints and `scripts/check-unsafe.py`; the patterns and their tests by review.
- **Arithmetic on values read from storage is checked.** *Check:* `clippy::arithmetic_side_effects`, and `scripts/check-arith-allows.py` for every allow.
- **Names and comments describe what the code does,** with no milestone, phase or task references. *Check:* `scripts/check-dev-notes.py`.

## Settings

- Settings are owned by the module that reads them and registered through a provider, not collected in one central list.
- Precedence: **index settings win over cluster settings, which win over node settings.**

*Check:* `Registry::new` refuses a duplicate key, a key outside its scope's prefix, an empty doc or a default it would refuse, and `the_nodes_settings_form_a_valid_registry` holds the node's whole registry to it ([how-to](how-to/add-a-setting.md)).

## Scale target

- About 10M tenants with about 10 indices each, on hundreds of nodes.
- Adaptive and fixed shard modes; small indices are auto-bucketed into shared shards.
- Anything that is O(total indices) on a hot path, a timer, or a subscription is a bug.

## Testing

- **Emulators only.** Tests run against the in-process S3 emulator, RustFS or the filesystem store, never real AWS. No Docker is required to build or test. *Check:* `scripts/check-test-endpoints.py`.
- **RustFS is for S3-compatibility checks only.** Perf, scale and lifecycle tests run on the in-process emulator or the fs store behind the same `BlobStore` trait, which meter requests the same way. A new test that reads `RUSTFS_BIN` must be listed, with a category, in `scripts/rustfs-scope.txt`. *Check:* `scripts/check-rustfs-scope.py`.
- **≥95% line coverage**, measured over every feature. *Check:* `scripts/coverage.sh --fail-under-lines 95` in the gate.
- **A gate nobody has seen fail is not trusted.** A new check is shown failing on the defect it targets before it lands, and its blind spots are written in its docstring and in [mechanical-gates.md](mechanical-gates.md).
- **Never weaken a threshold or delete a test to make a check pass.**
- **Never claim a test passed or a number was measured without having run it.** Numbers from emulators or shared hosts are labelled provisional.

## Pull requests

The process rules (one PR per change, self-merge after at most two review rounds, high-risk routing to an ADR and the owner, flaky-test merges, no AI attribution) have one home, the [`git-workflow` skill](../.agents/skills/git-workflow/SKILL.md). *Check:* `scripts/check-commit-msg.sh`, `scripts/check-pr.py`.
