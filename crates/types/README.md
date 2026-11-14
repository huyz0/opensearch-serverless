# oss-types

Identifiers and value types every other crate shares. Tenant, index, shard and node ids are distinct types so they cannot be mixed up.

## Owns
Shared identifiers and small value types. No I/O, no logic beyond validation.

## Depends on
Nothing in the workspace. Everything else depends on it.

## Entry points
`src/lib.rs`. Empty until the first role needs an id.
