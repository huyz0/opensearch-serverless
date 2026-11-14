# 0003: Apache License 2.0

**Status:** Accepted by the owner, 2026-11-14.

## Context

The repository is public. Without a licence, nobody may reuse the code. lucene-rust, the engine this builds on, is Apache-2.0, as are Lucene and OpenSearch.

## Decision

License the repository under the Apache License 2.0: `LICENSE` at the root, `license = "Apache-2.0"` in the workspace package, inherited by every crate.

## Consequences

- Matches lucene-rust, Lucene and OpenSearch, so code and ideas can move between them without a licence mismatch.
- Dependencies must stay Apache-2.0 compatible (MIT, BSD, ISC and similar permissive licences are fine).
