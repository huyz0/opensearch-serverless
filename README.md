# opensearch-serverless

A Rust serverless OpenSearch: stateless reader, writer and coordinator nodes over object storage, where S3 is the only durable tier and every S3 request is counted.

## Mission

1. **Truly compatible with OpenSearch** (REST API, query DSL, mappings, responses), scoped to the serverless capability. See [docs/compatibility.md](docs/compatibility.md).
2. **Ported clearly from existing code** (OpenSearch and the Java serverless shell in huyz0/OpenSearch, branch `feature/serverlessshell`) rather than written from scratch, with every Rust module traceable to the Java it ports. See [docs/porting.md](docs/porting.md).
3. **A cross-reference for [hsearch](https://github.com/huyz0/hsearch) and [pstore](https://github.com/huyz0/pstore)**, with lessons flowing both ways. Their code is not used here; [lucene-rust](https://github.com/huyz0/lucene-rust) is the engine and the only one of them used as code.

It shares the Java shell's blob formats, so the two can run against the same bucket while roles move to Rust one at a time: reader first, then writer, then coordinator.

**Design basis:** the assessment [lucene-rust, hsearch and pstore: what the serverless shell should do with them](https://claude.ai/artifact/8zB3WEAwtggQWDMzpe6uig) (2026-11-14), revised by [ADR 0002](docs/adr/0002-runtime-dependencies.md): lucene-rust only.

## Status

Foundation only: workspace, crate layout, gates and docs. No search or write path yet. See [docs/roadmap.md](docs/roadmap.md).

## Start

```bash
scripts/bootstrap.sh
```

No Docker needed, on Linux, macOS, Windows (Git Bash) or a Claude cloud sandbox. Details: [docs/cloud-setup.md](docs/cloud-setup.md).

## Read next

- [docs/architecture.md](docs/architecture.md): crate map, folder layout, read and write flows
- [docs/principles.md](docs/principles.md): the rules every change follows, and the checks that enforce them
- [docs/porting.md](docs/porting.md) and [docs/compatibility.md](docs/compatibility.md): where code comes from, and how compatibility is proven
- [docs/how-to/](docs/how-to/README.md): add a crate, an S3 call, a setting, a REST endpoint, an extension point
- [docs/adr/](docs/adr/README.md): decisions
- [AGENTS.md](AGENTS.md): orientation for AI agents (and a fast start for people)

## Licence

Apache License 2.0. See [LICENSE](LICENSE).
