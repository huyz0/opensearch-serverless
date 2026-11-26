# Restore: flags set to the value this node always applies

## Serves

[ADR 0045](../../adr/0045-shallow-snapshot-restore.md) refuses `partial`, `include_global_state`, `include_aliases` (until ADR 0065) and `ignore_unavailable` with 501 whatever their value. A client library that sends the flag at its default (`"partial": false`) is refused for a behaviour the node already has. A flag set to the value this node always applies is accepted and changes nothing; the other value stays refused, with its reason.

## Acceptance criteria

- AC-1: A restore body with `partial: false`, `include_global_state: false`, `include_aliases: false` or `ignore_unavailable: false` is accepted and restores exactly as one without the key. Test: `a_flag_at_the_value_this_node_applies_is_accepted`.
- AC-2: The same keys set to `true` are 501 `unsupported_restore` with the reason they had. Test: `what_the_shell_refuses_is_refused_with_its_reason`.
- AC-3: A flag that is not a JSON boolean (`"partial": "false"`) is 400 `illegal_argument_exception` naming the key. Test: `a_flag_that_is_not_a_boolean_is_400`.
- AC-4: Every other refused key (`alias_write_index_policy`, `settings`, `storage_type`, the source repository keys, `allow_no_indices`, `expand_wildcards`) stays refused whatever its value. Test: `a_refused_key_without_a_default_stays_refused`.
- AC-5: Over REST, a restore carrying the four flags at their values makes the same requests as one without them. Test: `flags_at_their_defaults_cost_nothing`.

## S3 request budget

None added: the body is read before the store is.

## Risks

`include_aliases: false` is accepted although core's default is true: a client that sends it asks for no aliases, which is what this node restores. A client that omits it expects aliases core would restore; that gap is unchanged and stays in docs/compatibility.md.

> Superseded for `include_aliases` and the `rename_alias_*` keys by [snapshot-aliases](../snapshot-aliases/SPEC.md) AC-4: they are read, not refused.
