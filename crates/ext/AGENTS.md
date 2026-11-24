# oss-ext: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- No dependency on another workspace crate and no runtime: this is the contract, not a host ([ADR 0052](../../docs/adr/0052-extension-modules-on-a-wasm-abi.md)).
- Nothing here allocates by a size a module reports; every length is checked against a ceiling first.
- A module is judged the same way whatever loads it.
