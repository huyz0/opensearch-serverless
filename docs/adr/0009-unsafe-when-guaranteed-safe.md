# 0009: `unsafe` in any crate, only when guaranteed safe

**Status:** Accepted by the owner, 2026-11-16 ("any crates can use unsafe but need to be in a safe manner"; "store it as a rule").

## Context

The workspace started with `unsafe_code = "forbid"` everywhere. The hot paths the roadmap needs fast, such as SIMD JSON tokenizing for bulk and search bodies, unchecked inner loops after one bounds check, and writes into preallocated buffers, cannot reach their speed in safe Rust alone. The owner chose the approach large Rust data engines use (Polars is the example given): `unsafe` contained in small, documented blocks behind safe APIs. That reference only helps a reader who knows Polars, so the rule spells out the patterns themselves.

## Decision

- Any crate may use `unsafe`, item by item, in the five patterns of [docs/unsafe.md](../unsafe.md). `unsafe impl Send/Sync`, `transmute`, extended lifetimes and FFI need the owner's written OK.
- The workspace lints move to `unsafe_code = "deny"` and `unsafe_op_in_unsafe_fn = "deny"`, with clippy `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` denied. `scripts/check-unsafe.py` refuses a module- or crate-wide allow, a manifest that relaxes the lint, and the owner-OK constructs off its approved list.
- Safe code comes first. `unsafe` is added only where a profile shows it pays, and the safe version stays as the fallback and the test oracle (Miri, fuzzing, differential property tests).

## Consequences

- Hot paths can use SIMD and unchecked access, and every such site is greppable (`allow(unsafe_code)`) and documented.
- Soundness now depends on review and tests, not only on the compiler. The first crate with an `unsafe` block adds a Miri run (nightly) and fuzz targets; until then they are checked by review.
- `git grep 'allow(unsafe_code)'` lists every site; one PR can no longer silently relax the whole crate.
