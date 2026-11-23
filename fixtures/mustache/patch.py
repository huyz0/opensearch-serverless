#!/usr/bin/env python3
"""Copies CustomMustacheFactory and CustomReflectionObjectHandler out of an
OpenSearch checkout and swaps the OpenSearch-only calls (XContentBuilder,
Iterables, CollectionUtils, the Jackson 3 encoder) for Jackson 2 and
Oracle.iterGet, which behave the same for the values a template reads.

usage: patch.py <opensearch checkout> <out dir>"""
import pathlib
import shutil
import sys

src = pathlib.Path(sys.argv[1]) / "modules/lang-mustache/src/main/java/org/opensearch/script/mustache"
out = pathlib.Path(sys.argv[2]) / "org/opensearch/script/mustache"
out.mkdir(parents=True, exist_ok=True)
for name in ("CustomMustacheFactory.java", "CustomReflectionObjectHandler.java"):
    shutil.copy(src / name, out / name)
shutil.copy(pathlib.Path(__file__).with_name("Oracle.java"), out / "Oracle.java")

f = out / "CustomMustacheFactory.java"
s = f.read_text()
s = s.replace("import org.opensearch.core.xcontent.MediaTypeRegistry;\n", "")
s = s.replace("import org.opensearch.core.xcontent.XContentBuilder;\n", "")
s = s.replace("import tools.jackson.core.io.JsonStringEncoder;", "import com.fasterxml.jackson.core.io.JsonStringEncoder;")
a = s.index("try (XContentBuilder builder")
b = s.index("} catch (IOException e) {", a)
s = s[:a] + '''try {
                    StringWriter sw = new StringWriter();
                    com.fasterxml.jackson.core.JsonGenerator g = new com.fasterxml.jackson.core.JsonFactory().createGenerator(sw);
                    com.fasterxml.jackson.databind.ObjectMapper m = new com.fasterxml.jackson.databind.ObjectMapper();
                    if (resolved instanceof Iterable<?> iterable) {
                        g.writeStartArray();
                        for (Object o : iterable) {
                            m.writeValue(g, o);
                        }
                        g.writeEndArray();
                    } else if (resolved instanceof Map<?, ?>) {
                        m.writeValue(g, resolved);
                    } else {
                        return oh.stringify(resolved);
                    }
                    g.flush();
                    return sw.toString();
                ''' + s[b:]
f.write_text(s)

h = out / "CustomReflectionObjectHandler.java"
s = h.read_text()
s = s.replace("import org.opensearch.common.util.iterable.Iterables;\n", "")
s = s.replace("import org.opensearch.core.common.util.CollectionUtils;\n", "")
s = s.replace("Iterables.get(", "Oracle.iterGet(")
s = s.replace('CollectionUtils.ensureNoSelfReferences(object, "CustomReflectionObjectHandler stringify");', "")
h.write_text(s)
