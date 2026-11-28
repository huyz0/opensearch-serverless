# Unsafe code

Decision: [ADR 0009](adr/0009-unsafe-when-guaranteed-safe.md).

Any crate may use `unsafe`, but only through the patterns below, each one small, private, documented and tested harder than safe code. The aim is speed on measured hot paths (SIMD parsing, unchecked inner loops) without giving up the guarantee that safe Rust gives: no undefined behaviour, whatever the input.

`unsafe` is never the first version. Write the safe version, measure it ([rust-performance](../.agents/skills/rust-performance/SKILL.md)), and add an unsafe path only where a profile shows it pays. The safe version stays, as the fallback and as the oracle the unsafe one is tested against.

## Allowed patterns

1. **Check once, then unchecked.** A safe entry point validates bounds, lengths or UTF-8 once, then the inner loop uses `get_unchecked`, `get_unchecked_mut` or `str::from_utf8_unchecked`. The check and the unchecked access are in the same module, and the SAFETY comment names the line that checked.
2. **Paired safe and unchecked functions.** A safe `fn take(..)` validates and calls a private `unsafe fn take_unchecked(..)`. The unchecked function's doc comment has a `# Safety` section listing every condition the caller must uphold. An `unsafe fn` is public only when another crate genuinely needs it, and then its `# Safety` section is the contract.
3. **SIMD intrinsics with runtime detection.** `std::arch` intrinsics live in `#[target_feature(enable = "...")]` functions. They are called only after `is_x86_feature_detected!` or `std::arch::is_aarch64_feature_detected!` says the CPU has the feature, or on a target where the feature is compiled in. A scalar version that gives identical results always exists and is what runs everywhere else.
4. **Writing into preallocated buffers.** Writing through a raw pointer or `spare_capacity_mut` into capacity reserved up front, with the length checked before the loop. `Vec::set_len` is called only after every element up to the new length is initialised.
5. **Reinterpreting bytes as plain-old-data.** Prefer `bytemuck` or `zerocopy`, which check size and alignment for you. A hand-written cast is allowed only between types with no padding and no invalid bit patterns (integers, floats, arrays of them), and it checks size and alignment first.

## Not allowed without the owner's written OK

- `unsafe impl Send` or `unsafe impl Sync`.
- `std::mem::transmute`, or any reinterpretation of a type that is not plain-old-data.
- Extending a lifetime (casting a borrow to `'static`, or keeping a pointer past its borrow).
- FFI (`extern` blocks, raw C calls) outside a crate built for it. The one owner-approved crate is `oss-ext-native` ([ADR 0080](adr/0080-one-plugin-system-three-tiers.md), 2026-11-28): it loads plugin libraries the operator allow-listed or signed, and its safety is that trust, not a proof.
- `unsafe` written to get past the borrow checker rather than for speed.

`scripts/check-unsafe.py` refuses the first, second and fourth outright. An approved exception is added to its allow list in the same PR that carries the owner's OK.

## Required on every use

- **Narrow allow.** The workspace lint is `unsafe_code = "deny"`. Each use takes `#[allow(unsafe_code)]` on the smallest item that needs it (the function, impl or block), never a whole module or crate (`#![allow(unsafe_code)]` is refused).
- **A SAFETY comment on every block:** `// SAFETY:` saying which invariant makes it sound and where that invariant was checked. One unsafe operation per block, so each comment covers exactly one thing.
- **A `# Safety` section on every `unsafe fn`.**
- **Private behind a safe API.** Callers outside the module see only safe functions.
- **`debug_assert!` of the invariant** at the top of each unchecked block, so debug builds and tests trip on a broken caller.
- **Tests aimed at the unsafe code:**
  - Miri runs every test of a module that contains `unsafe` (`cargo +nightly miri test -p <crate> <module>`).
  - A fuzz target per unsafe entry point.
  - A property test showing that the unsafe or SIMD path and the safe or scalar path agree on every input, plus an external oracle where one exists (`serde_json` for JSON).
  - The limit and one past it, as for any bounded resource.

## Enforced by

- `unsafe_code = "deny"` and `unsafe_op_in_unsafe_fn = "deny"` in the workspace lints. Every use needs a local allow, and an unsafe operation inside an `unsafe fn` still needs its own block.
- Clippy, denied in the workspace: `undocumented_unsafe_blocks` (no block or `unsafe impl` without `// SAFETY:`), `multiple_unsafe_ops_per_block`, and `missing_safety_doc` (through `clippy::all`).
- `scripts/check-unsafe.py`: refuses a module- or crate-wide allow, a manifest that relaxes `unsafe_code` below deny, `unsafe impl Send/Sync`, `transmute` and `extern` blocks, except entries on its owner-approved list.
- Not mechanical yet: that the allowed patterns are the ones used, that a SAFETY comment is true, and the Miri, fuzz and differential tests. The first crate with an `unsafe` block adds the Miri run to CI (nightly) and a fuzz target; until then, review checks them.
