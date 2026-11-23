# mustache fixtures

`cases.json` lists templates and params; `render.json` holds what mustache.java 0.9.14, as lang-mustache runs it, renders for each, errors included. `Oracle.java` and `patch.py` build the oracle; `record.sh <opensearch checkout> <work dir>` regenerates `render.json` (needs Java 17+ and Maven Central access). The reader's unit tests replay `render.json`.
