# opensearch-serverless

A Rust serverless OpenSearch: stateless reader, writer and coordinator nodes over object storage, where S3 is the only durable tier and every S3 request is counted.

It is developed in parallel with the Java serverless shell (huyz0/OpenSearch, branch `feature/serverlessshell`) and shares its blob formats, so the two can run against the same bucket while roles move to Rust one at a time: reader first, then writer, then coordinator. The engine is [lucene-rust](https://github.com/huyz0/lucene-rust); the runtime is [hsearch](https://github.com/huyz0/hsearch), with cost and catalog pieces from [pstore](https://github.com/huyz0/pstore).

**Design basis:** the assessment [lucene-rust, hsearch and pstore: what the serverless shell should do with them](https://claude.ai/artifact/8zB3WEAwtggQWDMzpe6uig) (2026-11-14).

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
- [docs/how-to/](docs/how-to/README.md): add a crate, an S3 call, a setting, a REST endpoint, an extension point
- [docs/adr/](docs/adr/README.md): decisions
- [AGENTS.md](AGENTS.md): orientation for AI agents (and a fast start for people)

## Licence

Apache License 2.0. See [LICENSE](LICENSE).
