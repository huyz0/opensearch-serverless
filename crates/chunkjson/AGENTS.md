# chunkjson: agent notes

Read [README.md](README.md) first, and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- **Extractable.** Depend on no crate of this workspace, not even as a dev-dependency or from `bench/` or `fuzz/` (`scripts/check-layering.py` refuses it); pin every external dependency exactly. The crate must build after `git filter-repo --subdirectory-filter crates/chunkjson` with only `[lints] workspace = true` replaced by the lint list.
- **Sans-IO.** The core never reads, writes, blocks or spawns. Adapters (`read.rs`, later async ones) sit on top of `Tokenizer::step`/`resolve`.
- **Bounded.** Nothing grows after `Tokenizer::new`: the stack and the number buffer are sized by `Config`. Strings stream as fragments and are never buffered.
- **serde_json is the oracle.** A grammar change keeps `tests/oracle.rs` and the fuzz target (`fuzz/`, nightly: `cargo +nightly fuzz run oracle`) agreeing with it, except the documented differences in the README.
- **Speed claims come from `bench/`** (`cargo run --release` there), run on the same host, labelled provisional.
