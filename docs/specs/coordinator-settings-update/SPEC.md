# The coordinator decides what a settings update may change

## Serves

Parity plan R7.4, index admin REST: the decisions in the Java shell's `SettingsUpdateHandler` (indexadmin package, shell 429b4cbc) that need no node: the shapes of the body, which keys are unknown, which asks are refused and in what order, and how the request merges over the settings the descriptor holds. The route, the core registry behind `Scope`, the value parse by that registry, the compare-and-swap loop and the reply are separate pieces.

## Change

`oss-coordinator` gains `indices::settings_update`: `Scope` (what the node's registry says of a key), `Kind`, `parse`, `merge` and `ATTEMPTS`.

## Acceptance criteria

- AC-1: no body or a blank one is refused 400 `missing_body`, "a settings body is required"; a body that is not a JSON object is refused 400 `malformed_body`, "could not parse the settings body: " and the reason; an object that flattens to nothing is refused 400 `missing_body`, "no settings were given".
- AC-2: both shapes are read: `{"settings": {...}}` when `settings` is an object, otherwise the object itself; keys are flattened and `index.`-prefixed as in create.
- AC-3: a key the scope calls ours is checked by the scope's own validator, and the first failure is refused 400 `illegal_argument_exception` with its words; a key the scope does not know (other than `index.serverless.shard_mode`, which AC-4 refuses) is collected, and the refusal is 400 `illegal_argument_exception`, "unknown index setting [a]" or "unknown index settings [a, b]", keys sorted.
- AC-4: refusals are 501 `unsupported_setting`, checked in this order, each in the shell's words: `index.serverless.shard_mode` given a value; `index.number_of_shards` given a value; `index.number_of_replicas` other than `0`; `index.search.default_pipeline` given a value; static core settings ("static index setting [k] cannot be changed on a live index..." with `settings` plural and a list when more than one), excluding the replica key.
- AC-5: the unknown check (AC-3) is decided before any refusal (AC-4).
- AC-6: `merge` layers the request over the current settings: a key in the request replaces the current value, a null removes the key, and the replica key never stays; with `preserve_existing` a key the current settings already hold a value for keeps it; the result is sorted by key; nothing changed gives none.
- AC-7: `ATTEMPTS` is 4.

## Test plan

`crates/coordinator/tests/settings_update.rs`: one test per criterion, a fake `Scope`, words asserted exactly.

## S3 request budget

Zero: nothing is read.

## Bounds

The tree and the flattened keys are bounded by the body, which the edge bounds.

## Switch

None: the shell's update has no setting and no measured downside.

## Risks

A null in the body removes the key here, which is core's meaning of resetting a setting; the shell stores the null through `Settings.Builder.put(key, null)`. Whether the descriptor drops or keeps it is the descriptor's rendering, a separate piece.
