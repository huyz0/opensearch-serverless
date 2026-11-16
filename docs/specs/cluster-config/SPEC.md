# Cluster config

## Serves

The plan's metadata-writes milestone, fifth part. An operator's persistent cluster settings live in one register, which Rust and Java nodes read and update alike. Follows [change-stripes](../change-stripes/SPEC.md).

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** the register changes only by conditional create or compare-and-swap, never by a lock or a leader.
- **Idle costs nothing:** nothing reads the register on a timer.

## Change

Decision records: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted) and [ADR 0011](../../adr/0011-cluster-config-flat-form.md) (Accepted after independent review), the flat form.

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/ClusterConfig.java` at `3920dc36` (`read`, `update`, `parse`, `toBytes`). Also ported are the core `Settings.fromXContent` and `Settings.Builder.build` (`processLegacyLists`) of OpenSearch 3.3.0, which those call.

### In `oss-format`

`ClusterSettings` is the register's value: a map from dotted key to `Setting::Value` or `Setting::List`.

- **`parse`** reads as `Settings.fromXContent`:
  - nested keys are joined with `.`;
  - a number keeps its text, and a boolean reads as `true` or `false`;
  - a later key replaces an earlier one;
  - lists hold scalars only;
  - `null` reads as absent.

  It counts the settings' flat size as it reads them: each key and value's bytes, plus 6 a setting and 3 a list entry. Past `MAX_CLUSTER_SETTINGS_BYTES` (256 KiB), it answers `FormatError::TooLarge` before reading on. Then, as the builder does, `p.0`, `p.1` ... become a list `p`. A `p.0` beside a `p` is refused. So is settings the Java holds but cannot read back: a null `p.0`, or a list made from `p.0.0`.
- **`write`** writes the `flat_settings` form into the caller's buffer, one field a key in key order.
- **`apply(changes)`** is the Java's `update` merge: a value sets its key and `None` removes it, and the result is then built the same way. It answers `SettingsRefused` and changes nothing in three cases:
  - the changes build a list, which the Java's REST handler refuses with the same message;
  - the result holds a `p.0` beside a `p`, where the Java's builder would throw;
  - the result is past the bound.
- **The settings are read-only** outside `parse` and `apply`, so every value held is one a Java node reads back.
- **The key:** `cluster_config_key(base)` gives `<base>cluster/config/settings`.

Where the Rust differs from the Java, on purpose:
- It writes the flat form; the Java writes the nested one in `HashMap` order (ADR 0011).
- The settings are bounded; the Java's are not.
- It accepts a field repeated in one object, keeping the last; Jackson refuses one. No writer writes one.
- It refuses bytes after the object; Jackson ignores them. No writer writes them.

### In `oss-meta`

`ClusterConfig` holds `read(buf) -> ConfigValue { generation, settings }` and `update(changes, buf) -> ClusterSettings`.

- **`read`** is one register read through the caller's buffer. Absent reads as `ABSENT_GENERATION` and no settings.
- **`update`** reads, merges, and then either creates the register or swaps it from the generation read. It makes up to 10 attempts with no pause, as the Java does, and then answers `MetaError::Contended`.
- **A refused update** answers `MetaError::SettingsRefused` after the read, before any write.
- **The buffer** bounds the value both ways. A value longer than it is refused: by the store on a read, and by `FormatError::TooLarge` on a write.

### Not in this change

The `_cluster/settings` REST handlers, `firstListValue` and `flatten`, which are REST helpers. Applying the settings to a running node.

## Acceptance criteria

- AC-1: Every case in `fixtures/cluster-config/cases.txt` reads as OpenSearch 3.3.0's `Settings` reads it. Rust writes the Java's `flat_settings` bytes for it (`parsed.txt`), which read back as the same settings. A case the Java refuses, or cannot read back, Rust refuses. Each nested value the Java's update stored (`updated.txt`) reads as the Java's flat form of it. Malformed bytes and short buffers are refused. Tests: `every_case_reads_as_the_java_reads_it`, `the_javas_nested_writes_read_as_its_flat_form`, `values_round_trip_with_escapes`, `a_short_buffer_says_how_long_it_must_be`, `malformed_values_are_refused`, `the_key_is_the_javas`.
- AC-2: `apply` merges as the Java's `update` over the steps in `updates.txt`, giving the Java's settings after each one. It refuses changes that build a list, and keeps a list already held through an unrelated update. Where the Java's builder throws, it answers `SettingsRefused` and leaves the settings unchanged. Settings at the bound are held; one byte past it is refused, by `apply` and by `parse`. So is a short nested value whose flat form is past the bound. Tests: `updates_merge_as_the_javas`, `an_update_that_makes_a_list_is_refused`, `lists_already_held_survive_an_unrelated_update`, `an_update_the_java_refuses_changes_nothing`, `settings_past_the_bound_are_refused`.
- AC-3: Requests, counted on each backend:
  - a `read` is 1 `Get`, whether or not the register exists;
  - the first `update` is 1 `Get` and 1 `Put`, a create;
  - a later one is 1 `Get` and 1 `Put`, plus 1 `GetRange` on fs, a swap;
  - each attempt lost to another writer repeats the `Get` and the swap, and the update answers `Contended` after 10, without pausing;
  - a refused update, or one too long for its buffer, is 1 `Get` and writes nothing.

  Tests: `nothing_set_reads_empty_in_one_request`, `updates_create_then_swap_and_merge`, `an_update_retries_while_others_move_the_register`, `an_update_the_java_would_refuse_writes_nothing`, `a_value_too_long_for_the_buffer_is_refused`.
- AC-4: A value a Java node stored reads on each backend, and an update writes the flat form a Java node reads back. A malformed value or base is refused. Tests: `a_value_a_java_node_stored_is_read_and_updated`, `a_malformed_value_or_base_is_refused`.

## Test plan

- AC-1 and AC-2: `crates/format/tests/cluster_settings.rs` against `fixtures/cluster-config/`. Those fixtures are written by `ClusterSettings.java`, which runs OpenSearch 3.3.0's `Settings` and the Java's `parse`, `toBytes` and update merge.
- AC-3 and AC-4: `crates/meta/tests/descriptors/cluster_config.rs` over the three backends, with exact meter counts. A store wrapper conflicts chosen swaps before they reach the store, so its counts leave out the lost swaps a real store bills.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `read` | 0 | 1 | 0 | 0 | 1 |
| `update`, uncontended | 1 | 1 (and 1 ranged on fs) | 0 | 0 | 2 |
| each lost attempt | +1 (a swap answered 412) | +1 | 0 | 0 | +2 |

Paid at the rate of operator settings changes. Nothing reads the register on a timer.

## Switch

None needed: nothing calls `ClusterConfig` yet. The REST handlers that will call it come with the coordinator.

## Risks

- An update lost to 10 other writers answers `Contended`, as in the Java. Operators write at human rates, so this needs about 10 concurrent writers.
- The flat form's bytes differ from the Java's nested form, which ADR 0011 accepts: both read as the same settings, and a swap compares generations.
- Settings past 256 KiB of flat text, which the Java would hold, do not read in Rust. Operator settings are a few kilobytes.
