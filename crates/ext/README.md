# oss-ext

The extension module contract that does not depend on how a module is loaded: the ABI version rule, the status codes, the ceilings, the strict descriptor parser and the typed failures of a call. The WASM host that runs a module is a separate crate.

## Owns
`version` (the packed ABI version and the rule that accepts a module's), `status` (the codes a module returns), `descriptor` (the strict parse of what a module says about itself), `error` (the typed failure of a call), the ceilings every host reads, and for plugins ([ADR 0080](../../docs/adr/0080-one-plugin-system-three-tiers.md)) `manifest` (the strict parse of a plugin's manifest and its tier check), `catalogue` (the points and the tiers that may serve each), `capability` (what a plugin may ask for, and the per-call budget) and `breaker` (the circuit breaker, over an injected clock).

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs`. Spec: [docs/specs/extension-abi](../../docs/specs/extension-abi/SPEC.md), [docs/specs/plugin-system](../../docs/specs/plugin-system/SPEC.md); decision: [ADR 0052](../../docs/adr/0052-extension-modules-on-a-wasm-abi.md).
