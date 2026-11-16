/*
 * Records what OpenSearch's own transport classes put on the wire, for
 * crates/transport/tests/. See README.md in this directory for how to run it.
 */
package org.opensearch.transport;

import org.opensearch.Version;
import org.opensearch.cluster.ClusterName;
import org.opensearch.cluster.node.DiscoveryNode;
import org.opensearch.cluster.node.DiscoveryNodeRole;
import org.opensearch.common.io.stream.BytesStreamOutput;
import org.opensearch.common.settings.Settings;
import org.opensearch.common.util.concurrent.ThreadContext;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.io.stream.StreamOutput;
import org.opensearch.core.common.io.stream.Writeable;
import org.opensearch.core.common.transport.TransportAddress;
import org.opensearch.core.tasks.TaskId;

import java.io.IOException;
import java.io.PrintWriter;
import java.lang.reflect.Constructor;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Random;
import java.util.Set;
import java.util.TreeMap;
import java.util.TreeSet;

public final class WireRecordings {

    /** The shell's version: what its nodes announce. */
    static final Version SHELL = Version.fromId(3100099 ^ Version.MASK);
    /** 3.10's minimum wire-compatible version, the last 2.x minor. */
    static final Version MIN_COMPAT = Version.fromId(2190099 ^ Version.MASK);

    interface Body {
        void write(StreamOutput out) throws IOException;
    }

    static String hex(BytesReference bytes) {
        final StringBuilder out = new StringBuilder();
        for (int i = 0; i < bytes.length(); i++) {
            out.append(String.format("%02x", bytes.get(i) & 0xff));
        }
        return out.toString();
    }

    static String hex(byte[] bytes) {
        final StringBuilder out = new StringBuilder();
        for (byte b : bytes) {
            out.append(String.format("%02x", b & 0xff));
        }
        return out.length() == 0 ? "-" : out.toString();
    }

    static String bytes(Body body) throws IOException {
        try (BytesStreamOutput out = new BytesStreamOutput()) {
            out.setVersion(SHELL);
            body.write(out);
            return hex(out.bytes());
        }
    }

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        stream(dir.resolve("stream.txt"));
        nodes(dir.resolve("nodes.txt"));
        frames(dir.resolve("frames.txt"));
        org.opensearch.OpenSearchServerException.registerExceptions();
        exceptions(dir.resolve("exceptions.txt"));
    }

    // ---- exceptions ----

    static <T extends Throwable> T bare(T t) {
        t.setStackTrace(new StackTraceElement[0]);
        return t;
    }

    static Exception remote(Throwable cause) throws IOException {
        final TransportAddress address = new TransportAddress(InetAddress.getByName("10.0.0.7"), 9300);
        return bare(new RemoteTransportException("w1", address, "internal:serverless/document/write", cause));
    }

    static void exceptions(Path file) throws Exception {
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(file))) {
            // kind, description, bytes of writeException; "bare" rows have no stack traces, so Rust writes them byte for byte.
            final Map<String, Exception> bare = new LinkedHashMap<>();
            bare.put("security:forwarded request without a valid transport token:FORBIDDEN", remote(bare(
                new org.opensearch.OpenSearchSecurityException("forwarded request without a valid transport token", org.opensearch.core.rest.RestStatus.FORBIDDEN))));
            bare.put("rejected:transport secret refresh is pending or rate limited; retry after one second", remote(bare(
                new org.opensearch.core.concurrency.OpenSearchRejectedExecutionException("transport secret refresh is pending or rate limited; retry after one second"))));
            bare.put("not_found:internal:serverless/search/shard", remote(bare(
                new ActionNotFoundTransportException("internal:serverless/search/shard"))));
            bare.put("status:shard [logs][3] is not owned here:SERVICE_UNAVAILABLE", remote(bare(
                new org.opensearch.OpenSearchStatusException("shard [logs][3] is not owned here", org.opensearch.core.rest.RestStatus.SERVICE_UNAVAILABLE))));
            for (Map.Entry<String, Exception> e : bare.entrySet()) {
                out.println("bare\t" + e.getKey() + "\t" + bytes(o -> o.writeException(e.getValue())));
            }
            // Read-only rows: stack traces, suppressed exceptions, causes of every JDK kind, headers and metadata.
            final IllegalArgumentException iae = new IllegalArgumentException("bad argument", new java.io.IOException("disk", new IllegalStateException("state")));
            iae.addSuppressed(new NullPointerException("npe"));
            final org.opensearch.OpenSearchStatusException withMeta = new org.opensearch.OpenSearchStatusException("with metadata", org.opensearch.core.rest.RestStatus.CONFLICT, iae);
            withMeta.addMetadata("opensearch.index", "logs");
            withMeta.addHeader("h", "1", "2");
            out.println("read\tstatus:with metadata:CONFLICT\t" + bytes(o -> o.writeException(remote(withMeta))));
            out.println("read\tjdk:6:bad argument\t" + bytes(o -> o.writeException(remote(iae))));
            out.println("read\tnone\t" + bytes(o -> o.writeException(null)));
            final Exception unknown = new org.opensearch.transport.ConnectTransportException(null, "no route");
            out.println("read\tunknown:12\t" + bytes(o -> o.writeException(remote(unknown))));
            // A throwable with no registry id or JDK key travels as NotSerializableExceptionWrapper.
            final Exception unregistered = new java.util.ConcurrentModificationException("changed underneath");
            out.println("read\twrapped:concurrent_modification_exception\t" + bytes(o -> o.writeException(remote(unregistered))));
            for (int key : new int[] { 1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18 }) {
                out.println("read\tjdk:" + key + "\t" + bytes(o -> o.writeException(remote(jdk(key)))));
            }
            // An error response frame, as the Java's outbound handler sends one.
            final Object response = response(context(Map.of(), Map.of()), Set.of(), (Writeable) remote(bare(
                new org.opensearch.OpenSearchSecurityException("forwarded request without a valid transport token", org.opensearch.core.rest.RestStatus.FORBIDDEN))),
                SHELL, 42, false, false);
            out.println("frame\tsecurity:forwarded request without a valid transport token:FORBIDDEN\t" + serialize(response));
        }
    }

    static Exception jdk(int key) {
        return switch (key) {
            case 1 -> new org.apache.lucene.index.CorruptIndexException("corrupt", "resource");
            case 2 -> new org.apache.lucene.index.IndexFormatTooNewException("res", 9, 1, 8);
            case 3 -> new org.apache.lucene.index.IndexFormatTooOldException("res", 1, 2, 8);
            case 4 -> new NullPointerException("npe");
            case 5 -> new NumberFormatException("nfe");
            case 7 -> new org.apache.lucene.store.AlreadyClosedException("closed");
            case 8 -> new java.io.EOFException("eof");
            case 9 -> new SecurityException("sec");
            case 10 -> new StringIndexOutOfBoundsException("sioobe");
            case 11 -> new ArrayIndexOutOfBoundsException("aioobe");
            case 12 -> new java.io.FileNotFoundException("fnf");
            case 13 -> new java.nio.file.AccessDeniedException("/a", "/b", "why");
            case 14 -> new IllegalStateException("ise");
            case 15 -> new org.apache.lucene.store.LockObtainFailedException("lock");
            case 16 -> new InterruptedException("int");
            case 17 -> new java.io.IOException("io");
            case 18 -> new org.opensearch.core.concurrency.OpenSearchRejectedExecutionException("rejected", true);
            default -> throw new IllegalArgumentException("key " + key);
        };
    }

    // ---- stream primitives ----

    static void stream(Path file) throws IOException {
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(file))) {
            final int[] ints = { 0, 1, 127, 128, 255, 16383, 16384, 2097151, 2097152, 268435455, 268435456, Integer.MAX_VALUE, -1, -128, Integer.MIN_VALUE };
            for (int v : ints) {
                out.println("vint\t" + v + "\t" + bytes(o -> o.writeVInt(v)));
                out.println("int\t" + v + "\t" + bytes(o -> o.writeInt(v)));
            }
            final long[] longs = { 0L, 1L, 127L, 128L, 16384L, 1L << 35, (1L << 56) - 1, 1L << 56, Long.MAX_VALUE };
            for (long v : longs) {
                out.println("vlong\t" + v + "\t" + bytes(o -> o.writeVLong(v)));
            }
            final long[] signed = { 0L, 1L, -1L, 63L, -64L, 64L, -65L, Long.MAX_VALUE, Long.MIN_VALUE, -2L, 1234567890123L, -1234567890123L };
            for (long v : signed) {
                out.println("zlong\t" + v + "\t" + bytes(o -> o.writeZLong(v)));
                out.println("long\t" + v + "\t" + bytes(o -> o.writeLong(v)));
            }
            out.println("bool\ttrue\t" + bytes(o -> o.writeBoolean(true)));
            out.println("bool\tfalse\t" + bytes(o -> o.writeBoolean(false)));
            for (Version v : new Version[] { SHELL, MIN_COMPAT, Version.fromId(3030099 ^ Version.MASK), Version.fromId(1000099 ^ Version.MASK) }) {
                out.println("version\t" + v.id + "\t" + bytes(o -> o.writeVersion(v)));
            }
            final Random random = new Random(16);
            final List<String> strings = new ArrayList<>(List.of("", "a", "internal:tcp/handshake", "é", "\u0000", "€", "😀", "日本語", "x".repeat(300)));
            for (int i = 0; i < 300; i++) {
                strings.add(randomString(random));
            }
            for (String s : strings) {
                out.println("string\t" + hex(s.getBytes(StandardCharsets.UTF_8)) + "\t" + bytes(o -> o.writeString(s)));
            }
            out.println("optional_string\t-\t" + bytes(o -> o.writeOptionalString(null)));
            out.println("optional_string\t" + hex("abc".getBytes(StandardCharsets.UTF_8)) + "\t" + bytes(o -> o.writeOptionalString("abc")));
            out.println("string_array\t-\t" + bytes(o -> o.writeStringArray(new String[0])));
            out.println("string_array\t" + hex("a,bc,é".getBytes(StandardCharsets.UTF_8)) + "\t" + bytes(o -> o.writeStringArray(new String[] { "a", "bc", "é" })));
            final Map<String, String> map = new TreeMap<>(Map.of("a", "1", "bb", "two", "é", ""));
            out.println("string_map\t" + hex("a=1,bb=two,é=".getBytes(StandardCharsets.UTF_8)) + "\t" + bytes(o -> o.writeMap(map, StreamOutput::writeString, StreamOutput::writeString)));
            out.println("task_id\t-\t" + bytes(o -> TaskId.EMPTY_TASK_ID.writeTo(o)));
            out.println("task_id\tnode-1:42\t" + bytes(o -> new TaskId("node-1", 42).writeTo(o)));
        }
    }

    static String randomString(Random random) {
        final int length = random.nextInt(40);
        final StringBuilder out = new StringBuilder();
        while (out.length() < length) {
            switch (random.nextInt(6)) {
                case 0 -> out.append((char) random.nextInt(0x80));
                case 1 -> out.append((char) (0x80 + random.nextInt(0x780)));
                case 2 -> out.append((char) (0x800 + random.nextInt(0xD000)));
                case 3 -> out.appendCodePoint(0x10000 + random.nextInt(0xFFFFF));
                case 4 -> out.append((char) (0xE000 + random.nextInt(0x2000)));
                default -> out.append((char) ('a' + random.nextInt(26)));
            }
        }
        return out.toString();
    }

    // ---- discovery nodes ----

    record NodeCase(String name, String id, String ephemeral, String hostName, String hostAddress, String ip, int port, String streamIp,
        Map<String, String> attributes, Set<DiscoveryNodeRole> roles, Version version) {
    }

    static List<NodeCase> nodeCases() {
        final List<NodeCase> cases = new ArrayList<>();
        cases.add(new NodeCase("w1", "w1", "Ab3dE-fGhIjKlMnOpQrStU", "10.0.0.7", "10.0.0.7", "10.0.0.7", 9300, null,
            Map.of("serverless.roles", "ingest,search"), new TreeSet<>(Set.of(DiscoveryNodeRole.DATA_ROLE, DiscoveryNodeRole.INGEST_ROLE)), SHELL));
        cases.add(new NodeCase("r-2", "r-2", "eph", "reader.local", "192.168.1.20", "192.168.1.20", 19300, null,
            Map.of("serverless.roles", "search"), new TreeSet<>(Set.of(DiscoveryNodeRole.DATA_ROLE)), SHELL));
        cases.add(new NodeCase("v6", "v6", "e6", "::1", "::1", "::1", 9301, null,
            Map.of(), new TreeSet<>(Set.of(DiscoveryNodeRole.DATA_ROLE)), SHELL));
        cases.add(new NodeCase("all", "all-id", "eall", "h", "127.0.0.1", "127.0.0.1", 1, "127.0.0.1",
            Map.of("zone", "us-east-1a"), new TreeSet<>(Set.of(DiscoveryNodeRole.CLUSTER_MANAGER_ROLE, DiscoveryNodeRole.DATA_ROLE,
                DiscoveryNodeRole.INGEST_ROLE, DiscoveryNodeRole.REMOTE_CLUSTER_CLIENT_ROLE, DiscoveryNodeRole.SEARCH_ROLE,
                dynamicRole("ml", "m", false))), SHELL));
        cases.add(new NodeCase("né", "日本", "😀", "h", "10.1.2.3", "10.1.2.3", 65535, null,
            Map.of(), new TreeSet<>(), MIN_COMPAT));
        return cases;
    }

    static DiscoveryNodeRole dynamicRole(String name, String abbreviation, boolean data) {
        try {
            return (DiscoveryNodeRole) construct("org.opensearch.cluster.node.DiscoveryNodeRole$DynamicRole",
                new Class<?>[] { String.class, String.class, boolean.class }, name, abbreviation, data);
        } catch (Exception e) {
            throw new IllegalStateException(e);
        }
    }

    static DiscoveryNode node(NodeCase c) throws IOException {
        final TransportAddress address = new TransportAddress(InetAddress.getByName(c.ip()), c.port());
        final TransportAddress stream = c.streamIp() == null ? null : new TransportAddress(InetAddress.getByName(c.streamIp()), c.port() + 1);
        return new DiscoveryNode(c.name(), c.id(), c.ephemeral(), c.hostName(), c.hostAddress(), address, stream, c.attributes(), c.roles(), c.version());
    }

    static String describe(NodeCase c) {
        final StringBuilder roles = new StringBuilder();
        for (DiscoveryNodeRole r : c.roles()) {
            if (roles.length() > 0) {
                roles.append(',');
            }
            roles.append(r.roleName()).append(':').append(r.roleNameAbbreviation()).append(':').append(r.canContainData());
        }
        final StringBuilder attrs = new StringBuilder();
        for (Map.Entry<String, String> e : new TreeMap<>(c.attributes()).entrySet()) {
            if (attrs.length() > 0) {
                attrs.append(';');
            }
            attrs.append(e.getKey()).append('=').append(e.getValue());
        }
        return String.join("\t", hex(c.name().getBytes(StandardCharsets.UTF_8)), hex(c.id().getBytes(StandardCharsets.UTF_8)),
            hex(c.ephemeral().getBytes(StandardCharsets.UTF_8)), c.hostName(), c.hostAddress(), c.ip(), Integer.toString(c.port()),
            c.streamIp() == null ? "-" : c.streamIp(), attrs.length() == 0 ? "-" : attrs.toString(), roles.length() == 0 ? "-" : roles.toString(),
            Integer.toString(c.version().id));
    }

    static void nodes(Path file) throws IOException {
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(file))) {
            for (NodeCase c : nodeCases()) {
                final DiscoveryNode n = node(c);
                out.println("with_attributes\t" + describe(c) + "\t" + bytes(n::writeToWithAttribute));
                out.println("plain\t" + describe(c) + "\t" + bytes(n::writeTo));
            }
        }
    }

    // ---- frames ----

    static final class Message extends TransportRequest {
        private final Body body;

        Message(TaskId parent, Body body) {
            this.body = body;
            setParentTask(parent);
        }

        @Override
        public void writeTo(StreamOutput out) throws IOException {
            super.writeTo(out);
            body.write(out);
        }
    }

    static final class Answer extends org.opensearch.core.transport.TransportResponse {
        private final Body body;

        Answer(Body body) {
            this.body = body;
        }

        @Override
        public void writeTo(StreamOutput out) throws IOException {
            body.write(out);
        }
    }

    static Object construct(String className, Class<?>[] types, Object... args) throws Exception {
        final Constructor<?> c = Class.forName(className).getDeclaredConstructor(types);
        c.setAccessible(true);
        return c.newInstance(args);
    }

    static String serialize(Object message) throws Exception {
        try (BytesStreamOutput out = new BytesStreamOutput()) {
            final var serialize = Class.forName("org.opensearch.transport.nativeprotocol.NativeOutboundMessage")
                .getDeclaredMethod("serialize", BytesStreamOutput.class);
            serialize.setAccessible(true);
            return hex((BytesReference) serialize.invoke(message, out));
        }
    }

    static Object request(ThreadContext context, String[] features, Writeable message, Version version, String action, long id,
        boolean handshake, boolean compress) throws Exception {
        return construct("org.opensearch.transport.nativeprotocol.NativeOutboundMessage$Request",
            new Class<?>[] { ThreadContext.class, String[].class, Writeable.class, Version.class, String.class, long.class, boolean.class, boolean.class },
            context, features, message, version, action, id, handshake, compress);
    }

    static Object response(ThreadContext context, Set<String> features, Writeable message, Version version, long id, boolean handshake,
        boolean compress) throws Exception {
        return construct("org.opensearch.transport.nativeprotocol.NativeOutboundMessage$Response",
            new Class<?>[] { ThreadContext.class, Set.class, Writeable.class, Version.class, long.class, boolean.class, boolean.class },
            context, features, message, version, id, handshake, compress);
    }

    static ThreadContext context(Map<String, String> request, Map<String, List<String>> response) {
        final ThreadContext context = new ThreadContext(Settings.EMPTY);
        request.forEach(context::putHeader);
        response.forEach((k, vs) -> vs.forEach(v -> context.addResponseHeader(k, v)));
        return context;
    }

    static String headers(Map<String, String> request, Map<String, List<String>> response) {
        final StringBuilder out = new StringBuilder();
        request.forEach((k, v) -> out.append(out.length() == 0 ? "" : ";").append("q:").append(k).append('=').append(v));
        response.forEach((k, vs) -> out.append(out.length() == 0 ? "" : ";").append("r:").append(k).append('=').append(String.join("|", vs)));
        return out.length() == 0 ? "-" : out.toString();
    }

    static void frames(Path file) throws Exception {
        try (PrintWriter out = new PrintWriter(Files.newBufferedWriter(file))) {
            // kind, request id, compress, headers, features, action, message description, hex
            final Writeable tcpRequest = (Writeable) construct("org.opensearch.transport.TransportHandshaker$HandshakeRequest",
                new Class<?>[] { Version.class }, MIN_COMPAT);
            final Writeable tcpResponse = (Writeable) construct("org.opensearch.transport.TransportHandshaker$HandshakeResponse",
                new Class<?>[] { Version.class }, SHELL);
            final Writeable serviceRequest = (Writeable) Class.forName("org.opensearch.transport.TransportService$HandshakeRequest")
                .getField("INSTANCE").get(null);
            for (boolean compress : new boolean[] { false, true }) {
                final String c = Boolean.toString(compress);
                out.println(String.join("\t", "tcp_handshake_request", "7", c, "-", "-", TransportHandshaker.HANDSHAKE_ACTION_NAME, "-",
                    serialize(request(context(Map.of(), Map.of()), new String[0], tcpRequest, MIN_COMPAT, TransportHandshaker.HANDSHAKE_ACTION_NAME, 7, true, compress))));
                out.println(String.join("\t", "tcp_handshake_response", "7", c, "-", "-", "-", "-",
                    serialize(response(context(Map.of(), Map.of()), Set.of(), tcpResponse, SHELL, 7, true, compress))));
                out.println(String.join("\t", "service_handshake_request", "8", c, "-", "-", TransportService.HANDSHAKE_ACTION_NAME, "-",
                    serialize(request(context(Map.of(), Map.of()), new String[0], serviceRequest, SHELL, TransportService.HANDSHAKE_ACTION_NAME, 8, false, compress))));
                int i = 0;
                for (NodeCase n : nodeCases()) {
                    final Writeable answer = new TransportService.HandshakeResponse(node(n), new ClusterName("serverless"), n.version());
                    out.println(String.join("\t", "service_handshake_response", Integer.toString(100 + i++), c, "-", "-", "-", describe(n),
                        serialize(response(context(Map.of(), Map.of()), Set.of(), answer, SHELL, 100 + i - 1, false, compress))));
                }
                final Map<String, String> one = new LinkedHashMap<>();
                one.put("x-serverless-transport-mac", "3:w1:1700000000000:00112233445566778899aabbccddeeff:" + "ab".repeat(32));
                final String text = "a document source that repeats, repeats, repeats, repeats, repeats".repeat(4);
                out.println(String.join("\t", "request", Long.toString(Long.MAX_VALUE), c, headers(one, Map.of()), "-", "internal:serverless/document/write",
                    "parent=-;string=" + hex(text.getBytes(StandardCharsets.UTF_8)),
                    serialize(request(context(one, Map.of()), new String[0], new Message(TaskId.EMPTY_TASK_ID, o -> o.writeString(text)), SHELL,
                        "internal:serverless/document/write", Long.MAX_VALUE, false, compress))));
                out.println(String.join("\t", "request", "0", c, "-", "feature-b,feature_a", "internal:serverless/commit/push",
                    "parent=w1:9;string=" + hex("x".getBytes(StandardCharsets.UTF_8)),
                    serialize(request(context(Map.of(), Map.of()), new String[] { "feature-b", "feature_a" },
                        new Message(new TaskId("w1", 9), o -> o.writeString("x")), SHELL, "internal:serverless/commit/push", 0, false, compress))));
                final Map<String, List<String>> warn = new LinkedHashMap<>();
                warn.put("Warning", List.of("299 OpenSearch \"deprecated\"", "second"));
                out.println(String.join("\t", "response", "-5", c, headers(Map.of(), warn), "-", "-", "string=" + hex(text.getBytes(StandardCharsets.UTF_8)),
                    serialize(response(context(Map.of(), warn), Set.of(), new Answer(o -> o.writeString(text)), SHELL, -5, false, compress))));
            }
        }
    }
}
