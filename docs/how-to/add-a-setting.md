# Add a setting

Settings live in `oss_types::settings` ([spec](../specs/settings-registry/SPEC.md)).

1. **Declare it next to the code that reads it**, as a `const` of one of `BoolSetting`, `IntSetting`, `LongSetting`, `StrSetting` or `ChoiceSetting`. Give it the Java's key, default, bounds or choices, a scope (`Scope::Node` or `Scope::Index`) and a doc. Add `.dynamic()` when it may change while a node runs.
2. **List it in the crate's provider**, `pub static SETTINGS: &[&Setting]` (see `crates/blob/src/settings.rs`). A crate's first setting adds its list to `providers()` in `crates/node/src/settings.rs`.
3. **Read it** with `get(source)`, where a source is any flat key-to-text lookup: the node's settings, the cluster register (`ClusterSettings`, or the `Live` snapshot), or an index's settings. A setting an index may override is a pair, `serverless.<name>` and `index.serverless.<name>`, read with `node_setting.resolve(&index_setting, &layers)`: **index overrides cluster, which overrides node**, and an empty value is unset.
4. **A switch is a setting:** off by default unless measured with no downside ([principles](../principles.md#switches-and-defaults)). Tests cover both values while the switch exists. Remove the setting once the default is proven.

**Checks:** `Registry::new` refuses a key defined twice, a key outside its scope's prefix, an empty doc, or a default the setting would refuse; `the_nodes_settings_form_a_valid_registry` (`crates/node`) holds every crate the node is built with to those rules. A cluster update is checked with `Registry::check_update`.
