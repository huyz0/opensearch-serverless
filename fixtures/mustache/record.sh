#!/bin/bash
# Records what the mustache engine renders for each case of cases.json into
# render.json, with mustache.java 0.9.14 and Jackson 2.17.2 as the oracle.
#
#   fixtures/mustache/record.sh <opensearch checkout> <work dir>
#
# The jars come from repo.maven.apache.org; the compiler jar's SHA-1 is the
# one OpenSearch pins in modules/lang-mustache/licenses.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
os="$1"
work="$2"
mkdir -p "$work/cls"
base=https://repo.maven.apache.org/maven2
fetch() {
  [ -s "$work/$2" ] || curl -fsS -o "$work/$2" "$base/$1"
}
fetch com/github/spullara/mustache/java/compiler/0.9.14/compiler-0.9.14.jar compiler.jar
fetch com/fasterxml/jackson/core/jackson-core/2.17.2/jackson-core-2.17.2.jar core.jar
fetch com/fasterxml/jackson/core/jackson-databind/2.17.2/jackson-databind-2.17.2.jar databind.jar
fetch com/fasterxml/jackson/core/jackson-annotations/2.17.2/jackson-annotations-2.17.2.jar annotations.jar
want="$(cat "$os/modules/lang-mustache/licenses/compiler-0.9.14.jar.sha1")"
[ "$(sha1sum "$work/compiler.jar" | cut -d' ' -f1)" = "$want" ]
python3 "$here/patch.py" "$os" "$work/src"
cp="$work/compiler.jar:$work/core.jar:$work/databind.jar:$work/annotations.jar"
javac -nowarn -d "$work/cls" -cp "$cp" "$work"/src/org/opensearch/script/mustache/*.java 2>/dev/null
java -Dfile.encoding=UTF-8 -Dstdout.encoding=UTF-8 -Dstdin.encoding=UTF-8 \
  -cp "$work/cls:$cp" org.opensearch.script.mustache.Oracle \
  < "$here/cases.json" 2>/dev/null \
  | python3 -c '
import json, sys
cases = json.load(open(sys.argv[1]))
results = json.load(sys.stdin)
out = []
for case, result in zip(cases, results):
    row = {"t": case["t"]}
    if "p" in case:
        row["p"] = case["p"]
    row.update(result)
    out.append(row)
print(json.dumps(out, indent=1, ensure_ascii=False))
' "$here/cases.json" > "$here/render.json"
