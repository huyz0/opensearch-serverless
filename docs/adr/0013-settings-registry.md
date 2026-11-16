# 0013: Settings as typed definitions, collected into one registry at build time

**Status:** Accepted after independent review, 2026-11-16.

## Context

Every switch the project ships is a setting: a cluster default, overridable per index ([principles](../principles.md#switches-and-defaults)). The Java shell declares each with `Setting.boolSetting`, `intSetting` and the rest, and collects them through `SettingsProvider`, loaded by `ServiceLoader` into `ServerlessSettings`. For a setting an index may override, it resolves index over cluster over node through one function, `LayeredSetting`, which skips a value that does not parse so the next layer decides: a bad cluster value cannot stop an index or a node from deciding. Cluster settings live in one register (ADR 0011), read again while a node runs.

Until now the Rust had one group of settings, `serverless.store.*`, parsed by its own module from a lookup function. Every role crate is about to add switches, so how they are declared, collected and read has to be settled once.

## Decision

- **A setting is a `const` in the crate that reads it**, of one of five typed kinds in `oss_types::settings`: bool, int, long, string, choice. Each carries the Java's key, default, bounds or choices, a scope (node or index), a dynamic flag and a doc. A kind parses as the Java's parser does and refuses with its message.
- **A crate lists its settings in `pub static SETTINGS: &[&Setting]`**, and `oss-node` collects the lists of the crates it is built with. This replaces `ServiceLoader`: what the binary holds is known at build time, and a crate behind a feature that is off brings no settings.
- **`Registry::new` checks the collected definitions**: no key defined twice, each key under its scope's prefix (`serverless.` or `index.serverless.`), a doc, a default the setting would accept. A test in `oss-node` runs it over the whole build. It also checks a cluster update with the Java's messages, so an unknown `serverless.` key, any `index.serverless.` key or a non-dynamic one is refused before it reaches the register.
- **Resolution is one function**: an index's value under `index.serverless.<name>`, else the register's under `serverless.<name>`, else the node's, else the default. An empty value is unset at every layer, as the Java's `""` defaults are. An index or cluster value that does not parse is skipped, as `LayeredSetting` skips it, so a Rust node and a Java node on one bucket decide alike; a bad node value is refused, as the Java refuses it at start-up. The answer names the layer.
- **Values that change while a node runs** are held as one immutable snapshot of the register, swapped whole under a lock only when a newer generation is read. A reader takes the snapshot and reads it with no lock held.
- Reading a setting allocates nothing; only a refusal builds its message.

## Consequences

- Adding a switch is a `const` and a list entry; a duplicate or a bad default fails a test, not a node in production.
- No new S3 request: the register is read as before (`ClusterConfig::read`). Refreshing the snapshot while a node runs comes with the node runtime: one read of the register a node an interval, as the Java's `refreshClusterPlacement`, never one an index.
- A choice setting refuses with one message format for all such settings, where the Java has one parser and message each; a setting whose Java message matters reads its value as text and keeps that message, as `serverless.store.type` does.
- Lists and time values are not kinds yet; they arrive with their first user.
- Plugins cannot add settings at run time. None run in the Rust node; if they ever do, a provider registered at start-up can feed the same `Registry::new`.
