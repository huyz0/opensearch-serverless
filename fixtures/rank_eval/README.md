# rank_eval fixtures

`map_order.txt` holds, for a capacity and keys put in order, the iteration order of the JDK's `HashMap(capacity)` and `ConcurrentHashMap(capacity)`, one case a line: `capacity|keys|hashmap order|concurrent order`. `MapOrder.java` records it: `java -Dfile.encoding=UTF-8 -Dstdout.encoding=UTF-8 MapOrder.java > map_order.txt`. The reader's unit tests replay it.
