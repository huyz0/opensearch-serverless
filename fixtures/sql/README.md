# SQL and PPL corpus, recorded from stock OpenSearch 3.10.0

`corpus.txt` lists 121 statements (`name|lang|statement`); `record.py` runs each against a stock 3.10.0 node with the SQL plugin over the indices of `data.json` and writes beside it, in `cases/`:

| File | Holds |
|---|---|
| `NAME.sql` / `NAME.ppl` | the statement |
| `NAME.jdbc.json` | the default (`jdbc`) answer |
| `NAME.dsl.json` | the plugin's `_explain` plan and, when the plan has a scan, the search body it sends (`dsl`) |
| `NAME.csv`, `NAME.raw` | the same statement with `format=csv` and `format=raw` |
| `NAME.error.json` | the status and body when the plugin refuses the statement |

Everything is recorded, nothing is written by hand. Re-record with `python3 fixtures/sql/record.py fixtures/sql` against a node started as in [fixtures/compat/README.md](../compat/README.md) (release tarball, security off). The plan's `pitId` and node-generated values are not normalised here; compare `dsl`, not `explain`.
