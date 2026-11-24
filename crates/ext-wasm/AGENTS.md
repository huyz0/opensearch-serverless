# oss-ext-wasm: agent notes

Read [README.md](README.md) first (what this crate owns, depends on, and its entry points), and the root [AGENTS.md](../../AGENTS.md) for the invariants.

Local rules:

- A module never gets more than the host's limits: a descriptor may only lower fuel, memory and deadline.
- Every byte a call can hold is reserved first (`call_reservation`); compiled code is held for as long as the module is loaded.
- A full gate or pool answers busy at once; nothing queues. A limit on one input is a 400, never a 429.
- A hostile module is a test in `tests/hostile.rs`; add one before any change to what a call may do.
- Upgrading `wasmtime` reruns those tests, because fuel counts can change.
