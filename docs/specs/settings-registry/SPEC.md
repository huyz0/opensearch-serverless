# Settings registry

## Serves

The plan's settings registry milestone: per-module settings with the Java's keys, types, defaults and messages; index over cluster over node; values that change while a node runs. Follows [index-lifecycle](../index-lifecycle/SPEC.md). Decision: [ADR 0013](../../adr/0013-settings-registry.md).

Invariants served:
- **Switches:** every switch is a setting declared once, next to the code that reads it, with a default and a doc.
- **Bounded memory:** reading a setting allocates nothing; only an error builds its message.

## Change

Ported from huyz0/OpenSearch at `3920dc36`:
- `serverless/shell/src/main/java/org/opensearch/serverless/settings/`: `SettingsProvider` and `ServerlessSettings` (`collect`, `indexScoped`, `unknownNodeKeys`).
- `server/src/main/java/org/opensearch/common/settings/Setting.java`: `boolSetting`, `intSetting`, `longSetting` and `simpleString`, with `parseInt`, `parseLong` and `parseBoolean` and their messages.
- `serverless/shell/src/main/java/org/opensearch/serverless/settings/LayeredSetting.java`: the index-over-cluster-over-node lookup that `SegmentPlacement`, `BucketPolicy` and the tail overlays share.

### In `oss-types`

- **`settings::{BoolSetting, IntSetting, LongSetting, StrSetting, ChoiceSetting}`**: const definitions of one setting each, with a key, a default, bounds or choices, a scope (node or index), whether it is dynamic, and a doc. Each one's `get(source)` parses the value, or returns the default when the key is absent. A value that does not parse is refused with the Java's message. A value is never trimmed, as in the Java.
- **`Source`**: a flat key-to-text lookup. `BTreeMap<String, String>`, `HashMap<String, String>` and a slice of pairs (`&[(&str, &str)]`) implement it, and `oss-format`'s `ClusterSettings` does too.
- **`Layers { index, cluster, node }` and each setting's `resolve(index_setting, layers)`**: the index value under the index key, else the cluster register's under the setting's key, else the node's, else the default. An empty value is unset at every layer, as the shell's `""` defaults are. An index or cluster value that does not parse is skipped, so the next layer decides; a node value that does not parse is refused. The answer names the layer it came from.
- **`Registry::new(providers)`**: collects every provider's settings and refuses:
  - a key defined twice, naming it;
  - a node key outside `serverless.`, or an index key outside `index.serverless.`;
  - an empty doc;
  - a default outside its bounds or choices.

  It also has `get(key)`, `index_scoped()`, `unknown_node_keys(keys)` (sorted, as the Java's) and `check_update(key, value)`. The last refuses, with `AbstractScopedSettings.validate`'s messages, an unknown `serverless.` key, any `index.serverless.` key, a key that is not dynamic, or a value that does not parse. Other keys pass through, since they belong to core and plugins. The Java's "did you mean" suggestion is not made.
- **`Live`**: the node's copy of the cluster register's settings, swapped whole. `replace(snapshot)` applies only a newer generation, so a read that lost a race changes nothing. `current()` hands out the snapshot it holds, with no lock held while it is read.

### Elsewhere

- `oss-blob`'s `serverless.store.*` keys become definitions, collected as `oss_blob::settings::SETTINGS`. `StoreSettings::parse` reads through them, and its messages are unchanged.
- `oss-node`'s `settings::registry()` collects the providers of the crates the node is built with. A test there holds the whole registry to the rules above.
- `docs/how-to/add-a-setting.md` describes what now exists.

### Not in this change

The settings REST endpoints, and refreshing `Live` from the register on a timer, come with the node runtime. Settings for code not yet ported arrive with that code. Lists (`serverless.plugins`) and time values arrive with their first user.

## Acceptance criteria

- AC-1: Each kind parses as the Java does and refuses with its message: `true`/`false` only; an int or long within its bounds, with `must be >=` and `must be <=`; a value that is no number; a choice outside its list. An absent key gives the default. Tests: `a_bool_is_true_or_false_and_nothing_else`, `numbers_parse_within_their_bounds_with_the_java_messages`, `a_choice_is_one_of_its_list`, `an_absent_key_is_the_default_and_a_present_one_is_never_trimmed`.
- AC-2: Resolution takes index over cluster over node over default, treats an empty value as unset at every layer, names the layer, skips an index or cluster value that does not parse so the next layer decides (the Java's `LayeredSetting` cases), and refuses a bad node value. Tests: `index_overrides_cluster_overrides_node`, `an_empty_value_is_unset_at_every_layer`, `a_bad_value_is_skipped_and_the_next_layer_decides`.
- AC-3: The registry refuses a duplicate key (naming it), a key outside its scope's prefix, an empty doc, and a default outside its bounds or choices. It finds keys, lists index-scoped settings, reports unknown `serverless.` keys sorted, and checks a cluster update, refusing any `index.serverless.` key. Tests: `a_registry_refuses_a_key_defined_twice`, `a_registry_refuses_a_key_outside_its_prefix_or_without_a_doc`, `a_registry_refuses_a_default_it_would_refuse_as_a_value`, `a_registry_finds_keys_and_reports_unknown_ones`, `an_update_is_checked_against_the_registry`, `an_index_setting_is_no_cluster_update`.
- AC-4: `Live` applies only newer generations. A snapshot taken before a swap keeps its values. Tests: `live_settings_take_only_newer_generations`, `a_snapshot_outlives_the_swap`.
- AC-5: The store settings read through their definitions with unchanged messages, the cluster register is a source, and the node's whole registry passes the rules. Tests: `fs_is_the_default_and_needs_a_path`, `s3_needs_a_bucket_and_defaults_the_rest`, `cluster_settings_are_a_source`, `the_nodes_settings_form_a_valid_registry`.

## Test plan

`crates/types/tests/settings/` for the kinds, layers, registry and live settings; `crates/format/tests/cluster_settings.rs` for the source; `crates/node` for the registry. Each refusal is checked against the Java's message text.

## S3 request budget

Unchanged: no request. Reading the register stays `ClusterConfig::read`.

## Switch

None: this is the mechanism switches use.

## Risks

- A setting declared in Rust with a default other than the Java's would let the two runtimes behave differently on one bucket. Each definition names its Java source, and the port map holds the pairing.
