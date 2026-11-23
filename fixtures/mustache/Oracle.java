package org.opensearch.script.mustache;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.github.mustachejava.Mustache;
import java.io.StringReader;
import java.io.StringWriter;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Collections;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Renders each {"t": template, "p": params} of stdin as the mustache engine does. */
public class Oracle {
    /** org.opensearch.common.util.iterable.Iterables.get for a list. */
    static Object iterGet(Collection<Object> c, int position) {
        if (position < 0) {
            throw new IllegalArgumentException("position >= 0");
        }
        Iterator<Object> it = c.iterator();
        for (int index = 0; index < position; index++) {
            if (!it.hasNext()) {
                throw new IndexOutOfBoundsException(Integer.toString(position));
            }
            it.next();
        }
        if (!it.hasNext()) {
            throw new IndexOutOfBoundsException(Integer.toString(position));
        }
        return it.next();
    }

    @SuppressWarnings("unchecked")
    public static void main(String[] a) throws Exception {
        ObjectMapper m = new ObjectMapper();
        List<Map<String, Object>> cases = m.readValue(System.in, List.class);
        List<Map<String, Object>> res = new ArrayList<>();
        for (Map<String, Object> c : cases) {
            Map<String, Object> r = new LinkedHashMap<>();
            try {
                CustomMustacheFactory f = new CustomMustacheFactory("application/json; charset=UTF-8");
                Mustache mu = f.compile(new StringReader((String) c.get("t")), "query-template");
                StringWriter w = new StringWriter();
                Object p = c.get("p");
                mu.execute(w, p == null ? Collections.emptyMap() : p);
                r.put("out", w.toString());
            } catch (Throwable e) {
                r.put("err", e.getClass().getSimpleName() + ": " + e.getMessage());
            }
            res.add(r);
        }
        System.out.println(m.writeValueAsString(res));
    }
}
