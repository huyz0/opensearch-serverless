# oss-types

Identifiers and value types every other crate shares. Tenant, index, shard and node ids are distinct types so they cannot be mixed up.

## Owns
Shared identifiers and small value types, and the settings definitions every module declares its settings with (`settings`): typed parsing with the Java's messages, index-over-cluster-over-node resolution, the registry and the node's live cluster settings. No I/O, no logic beyond validation.

## Depends on
Nothing in the workspace. Everything else depends on it.

## Entry points
`src/lib.rs`; `src/settings/` (`BoolSetting`, `IntSetting`, `LongSetting`, `StrSetting`, `ChoiceSetting`, `Layers`, `Registry`, `Live`). Spec: [docs/specs/settings-registry](../../docs/specs/settings-registry/SPEC.md).
