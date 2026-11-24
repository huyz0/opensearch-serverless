# oss-ext-wasm

The WASM host for extension modules: it validates, compiles and describes a module, and runs one call at a time in a fresh, bounded instance. No extension point is wired to a request path here.

## Owns
`host` (the engine, its deadline ticker and loading), `module` (a described module and its call), `validate` (what is refused before any compile), `gate` (the per-module and per-tenant concurrency gate), `reserve` (the pool a caller supplies), `config` (the host's limits).

## Depends on
`oss-ext` (the contract), `wasmtime` and `wasmparser` (pinned: fuel counts can change with a version).

## Entry points
`Host::load` and `Module::call` in `src/lib.rs`. Spec: [docs/specs/extension-abi](../../docs/specs/extension-abi/SPEC.md); decision: [ADR 0052](../../docs/adr/0052-extension-modules-on-a-wasm-abi.md).
