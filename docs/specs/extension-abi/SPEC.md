# Extension modules: the ABI and the WASM host

## Serves

R15.3, decided in [ADR 0052](../../adr/0052-extension-modules-on-a-wasm-abi.md): sandboxed WASM modules on a bytes-in, bytes-out ABI, content-addressed in the store. The rules a point follows are in [add-an-extension-point](../../how-to/add-an-extension-point.md). No point is wired to a request path here.

## Change

Slice 1 (merged): `oss-ext`, the ABI with no runtime dependency: the version rule, status codes, ceilings, the strict descriptor parser and the typed failures.

Slice 2, this PR: `oss-ext-wasm`, the host: wasmtime, the limits, a reservation trait the caller supplies, and hostile modules tested through a test-only module builder. The `Extensions` pool itself arrives with slice 3, which wires the trait to it.

Later slice, its own PR with its own `VERIFIED.md` rows: the store and the administrator upload route behind `serverless.extensions.enabled`.

### Guest contract the host reads

A module imports only `oss.log(ptr, len)` and `oss.cancelled() -> i32` and exports `memory`, `oss_abi_version() -> i32`, `oss_describe() -> i32` (the descriptor's length, written at the start of the output region), `oss_input_region() -> i32`, `oss_output_region() -> i32`, `oss_result_len() -> i32` and `oss_call(point, in_len, out_cap) -> i32`. Both regions must lie in memory, not overlap, and be `io_bytes` long. `cancelled` turns true at half the deadline; the epoch deadline ends the call by force at the whole of it, in steps of 10 ms.

Not in this change: any point, pins, native modules, the write or search path.

## Acceptance criteria

- AC-1: A module is accepted only when its ABI major equals the host's and its minor is at most the host's; a newer minor or another major is refused, naming which. A refused module is judged on the version alone.
- AC-2: A descriptor of at most 16 KiB is parsed strictly: unknown keys, duplicate keys, a missing key, a wrong type, a bad name, version or id, an empty point list, a duplicate point and a descriptor over the ceiling are each refused with a typed failure naming the key.
- AC-3: A status code outside the defined set is malformed; zero is success, every failure is negative; `buffer too small` is final (the call fails, no retry); a module's detail is cut to 256 bytes and its control characters replaced; capacity is 429, a module that broke the contract 502, any other failure of the input 400.
- AC-4: The ceilings (16 KiB descriptor, 64 KiB module configuration, 8 MiB module, 16 MiB I/O region) are constants the parser and the host read; a point and a descriptor's `limits` may only lower them, and a zero limit is refused.
- AC-5 (slice 2): A hostile module (spin, memory growth, trap, an oversized output, a region out of bounds) fails its call with a typed failure and leaves the node serving; a limit is a 400 `extension_failed`, capacity is a 429.
- AC-6 (slice 2): Calls reserve `linear memory + 2 x io_bytes + stack` and compiled code is charged to the `Extensions` pool; a pool that cannot hold it answers 429; the peak never passes the reservation.
- AC-7 (slice 3): A module is one PUT under its hash ever and one GET per module per node, none per call; an upload over 8 MiB is refused, a second concurrent upload answers 429, a repeat adds no PUT, and the janitor never touches `ext/`.
- AC-8 (slice 3): With `serverless.extensions.enabled` off nothing is loaded, no pool is reserved and the upload route does not exist.
