---
name: unsafe-code
description: "WHAT: When and how unsafe Rust is allowed: five patterns, the forbidden list, and the guards every use needs. USE WHEN: writing or reviewing any `unsafe` block, `unsafe fn`, `#[allow(unsafe_code)]`, SIMD intrinsics (`std::arch`), unchecked indexing, or byte reinterpretation."
---

# Unsafe code

The standard is [docs/unsafe.md](../../../docs/unsafe.md); it wins on conflict.

## Rules

- **Safe first, measured second.** Write the safe version, profile it, and add `unsafe` only where the numbers show it pays. The safe version stays as the fallback and the test oracle.
- **Only the five patterns:** check once then unchecked; paired safe and `_unchecked` functions; SIMD behind runtime feature detection with an identical scalar path; writes into preallocated buffers with a checked length; byte reinterpretation of plain-old-data, preferably via `bytemuck` or `zerocopy`.
- **Never without the owner's OK:** `unsafe impl Send/Sync`, `transmute`, extending lifetimes, FFI outside a crate built for it, `unsafe` to silence the borrow checker.
- **Every use:** `#[allow(unsafe_code)]` on the smallest item; a `// SAFETY:` comment per block naming the invariant and where it was checked; one unsafe operation per block; `# Safety` docs on each `unsafe fn`; private behind a safe API; `debug_assert!` of the invariant.
- **Tests:** Miri over the module, a fuzz target per unsafe entry point, and a property test where the unsafe path and the safe path (and an oracle such as `serde_json`) agree.

## Enforced by

- Workspace lints: `unsafe_code = "deny"`, `unsafe_op_in_unsafe_fn = "deny"`, clippy `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `missing_safety_doc`.
- `scripts/check-unsafe.py`: no module- or crate-wide allow, no relaxed manifest, no `unsafe impl Send/Sync`, `transmute` or `extern` block off its approved list.
- Not mechanical yet: that the pattern is one of the five, that the SAFETY comment is true, the Miri, fuzz and differential tests. Review checks them.
