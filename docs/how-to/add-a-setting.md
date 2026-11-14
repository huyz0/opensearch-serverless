# Add a setting

The settings provider does not exist yet; the first setting builds it to this shape.

- A setting is declared in the module that reads it, next to the code, not in a central list. The module registers its settings through a provider the node collects at start-up.
- Each setting has a key, a type, a default, a scope (node, cluster, index) and a doc comment.
- Resolution: **index overrides cluster, which overrides node.**
- A feature switch is a setting: off by default unless measured with no downside ([principles](../principles.md#switches-and-defaults)). Tests cover both values while the switch exists. Remove the setting once the default is proven.

**Checks:** none mechanical yet. When the provider exists, a test will assert every registered key is unique and documented.
