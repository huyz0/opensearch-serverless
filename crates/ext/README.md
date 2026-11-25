# oss-ext

The extension module contract that does not depend on how a module is loaded: the ABI version rule, the status codes, the ceilings, the strict descriptor parser and the typed failures of a call. The WASM host that runs a module is a separate crate.

## Owns
`version` (the packed ABI version and the rule that accepts a module's), `status` (the codes a module returns), `descriptor` (the strict parse of what a module says about itself), `error` (the typed failure of a call), and the ceilings every host reads.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs`. Spec: [docs/specs/extension-abi](../../docs/specs/extension-abi/SPEC.md); decision: [ADR 0052](../../docs/adr/0052-extension-modules-on-a-wasm-abi.md).
