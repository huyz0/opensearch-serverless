# SMILE documents written by Jackson

Each case is `<name>.json` (the JSON a Jackson `JsonGenerator` wrote) beside `<name>.m0.smile`, `.m1.smile` and `.m2.smile` (the same calls through Jackson's `SmileGenerator` with shared names; shared names and values; and shared names and values with 7-bit binary). `GenSmile.java` writes them against Jackson 3.2.3's `jackson-core` and `jackson-dataformat-smile` (the wire format is the one the Jackson 2.x OpenSearch ships writes):

```
javac -cp jackson-core-3.2.3.jar:jackson-dataformat-smile-3.2.3.jar GenSmile.java
java  -cp jackson-core-3.2.3.jar:jackson-dataformat-smile-3.2.3.jar:. GenSmile fixtures/snapshots/stock-smile
```

The cases cover every value type, long and unicode strings, binary of every tail length, shared names and strings (including past 31 and past 1,024 entries, where the table restarts) and long names. Test: `crates/format/tests/smile_fixtures.rs`. They are not documents a stock OpenSearch node wrote; a recorded stock snapshot joins them when the repository reader lands.
