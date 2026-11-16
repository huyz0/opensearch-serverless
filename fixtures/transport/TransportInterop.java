/*
 * A real OpenSearch transport (Netty4Transport and TransportService, at the
 * shell's version id) talking to the Rust transport over loopback, both
 * ways. See README.md in this directory for how to run it with
 * crates/transport/examples/interop.rs.
 */
package org.opensearch.transport;

import org.opensearch.OpenSearchStatusException;
import org.opensearch.Version;
import org.opensearch.cluster.node.DiscoveryNode;
import org.opensearch.cluster.node.DiscoveryNodeRole;
import org.opensearch.common.network.NetworkService;
import org.opensearch.common.settings.ClusterSettings;
import org.opensearch.common.settings.Settings;
import org.opensearch.common.util.PageCacheRecycler;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.core.common.io.stream.NamedWriteableRegistry;
import org.opensearch.core.common.io.stream.StreamInput;
import org.opensearch.core.common.io.stream.StreamOutput;
import org.opensearch.core.common.transport.TransportAddress;
import org.opensearch.core.indices.breaker.NoneCircuitBreakerService;
import org.opensearch.core.rest.RestStatus;
import org.opensearch.core.transport.TransportResponse;
import org.opensearch.telemetry.tracing.noop.NoopTracer;
import org.opensearch.threadpool.ThreadPool;
import org.opensearch.transport.netty4.Netty4Transport;

import java.io.IOException;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.util.Collections;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.TimeUnit;

public final class TransportInterop {

    static final Version SHELL = Version.fromId(3100099 ^ Version.MASK);
    static final String ECHO = "interop:echo";
    static final String FAIL = "interop:fail";

    /** A parent task id, then a byte reference. */
    static final class EchoRequest extends TransportRequest {
        final BytesReference payload;

        EchoRequest(BytesReference payload) {
            this.payload = payload;
        }

        EchoRequest(StreamInput in) throws IOException {
            super(in);
            payload = in.readBytesReference();
        }

        @Override
        public void writeTo(StreamOutput out) throws IOException {
            super.writeTo(out);
            out.writeBytesReference(payload);
        }
    }

    /** The byte reference back. */
    static final class EchoResponse extends TransportResponse {
        final BytesReference payload;

        EchoResponse(BytesReference payload) {
            this.payload = payload;
        }

        EchoResponse(StreamInput in) throws IOException {
            payload = in.readBytesReference();
        }

        @Override
        public void writeTo(StreamOutput out) throws IOException {
            out.writeBytesReference(payload);
        }
    }

    static TransportService service(ThreadPool pool, boolean compress) {
        final Settings settings = Settings.builder()
            .put("node.name", "java1")
            .put("cluster.name", "interop")
            .put("transport.port", "0")
            .put("transport.host", "127.0.0.1")
            .put("transport.compress", compress)
            .build();
        final Netty4Transport transport = new Netty4Transport(
            settings,
            SHELL,
            pool,
            new NetworkService(Collections.emptyList()),
            PageCacheRecycler.NON_RECYCLING_INSTANCE,
            new NamedWriteableRegistry(Collections.emptyList()),
            new NoneCircuitBreakerService(),
            new SharedGroupFactory(settings),
            NoopTracer.INSTANCE
        );
        final TransportService service = new TransportService(
            settings,
            transport,
            pool,
            TransportService.NOOP_TRANSPORT_INTERCEPTOR,
            bound -> new DiscoveryNode(
                "java1",
                "java-node",
                "java-eph",
                "127.0.0.1",
                "127.0.0.1",
                bound.publishAddress(),
                Map.of(),
                Set.of(DiscoveryNodeRole.DATA_ROLE),
                SHELL
            ),
            new ClusterSettings(settings, ClusterSettings.BUILT_IN_CLUSTER_SETTINGS),
            Collections.emptySet(),
            NoopTracer.INSTANCE
        );
        service.registerRequestHandler(
            ECHO,
            ThreadPool.Names.GENERIC,
            EchoRequest::new,
            (request, channel, task) -> channel.sendResponse(new EchoResponse(request.payload))
        );
        service.registerRequestHandler(
            FAIL,
            ThreadPool.Names.GENERIC,
            EchoRequest::new,
            (request, channel, task) -> channel.sendResponse(
                new OpenSearchStatusException("java says not now", RestStatus.SERVICE_UNAVAILABLE)
            )
        );
        service.start();
        service.acceptIncomingRequests();
        return service;
    }

    static CompletableFuture<EchoResponse> send(TransportService service, DiscoveryNode node, String action, String text) {
        final CompletableFuture<EchoResponse> future = new CompletableFuture<>();
        service.sendRequest(
            node,
            action,
            new EchoRequest(new BytesArray(text.getBytes(StandardCharsets.UTF_8))),
            new TransportResponseHandler<EchoResponse>() {
                @Override
                public EchoResponse read(StreamInput in) throws IOException {
                    return new EchoResponse(in);
                }

                @Override
                public void handleResponse(EchoResponse response) {
                    future.complete(response);
                }

                @Override
                public void handleException(TransportException exp) {
                    future.completeExceptionally(exp);
                }

                @Override
                public String executor() {
                    return ThreadPool.Names.SAME;
                }
            }
        );
        return future;
    }

    static void check(boolean ok, String what) {
        if (!ok) {
            throw new AssertionError(what);
        }
        System.out.println("ok " + what);
    }

    static Throwable failure(CompletableFuture<EchoResponse> future) throws Exception {
        try {
            future.get(10, TimeUnit.SECONDS);
            return null;
        } catch (java.util.concurrent.ExecutionException e) {
            return e.getCause();
        }
    }

    /** Calls the Rust node at `port`, whose ephemeral id is `eph`. */
    static void call(ThreadPool pool, int port, String eph, boolean compress) throws Exception {
        final TransportService service = service(pool, compress);
        try {
            final DiscoveryNode rust = new DiscoveryNode(
                "w1",
                "node-1",
                eph,
                "127.0.0.1",
                "127.0.0.1",
                new TransportAddress(InetAddress.getLoopbackAddress(), port),
                Map.of(),
                Set.of(DiscoveryNodeRole.DATA_ROLE),
                SHELL
            );
            service.connectToNode(rust);
            check(service.nodeConnected(rust), "java connected to the rust node (compress=" + compress + ")");
            final EchoResponse echoed = send(service, rust, ECHO, "hello from java").get(10, TimeUnit.SECONDS);
            check(echoed.payload.utf8ToString().equals("hello from java"), "rust echoed java's request");
            final Throwable missing = failure(send(service, rust, "interop:missing", "x"));
            check(
                missing instanceof RemoteTransportException
                    && missing.getCause() instanceof ActionNotFoundTransportException
                    && missing.getCause().getMessage().equals("No handler for action [interop:missing]"),
                "rust's ActionNotFound reads as java's: " + missing
            );
            final Throwable failed = failure(send(service, rust, FAIL, "x"));
            check(
                failed instanceof RemoteTransportException
                    && failed.getCause() instanceof OpenSearchStatusException
                    && ((OpenSearchStatusException) failed.getCause()).status() == RestStatus.SERVICE_UNAVAILABLE,
                "rust's status failure reads as java's: " + failed
            );
            final DiscoveryNode impostor = new DiscoveryNode(
                "w1",
                "node-1",
                "eph-other",
                "127.0.0.1",
                "127.0.0.1",
                new TransportAddress(InetAddress.getLoopbackAddress(), port),
                Map.of(),
                Set.of(DiscoveryNodeRole.DATA_ROLE),
                SHELL
            );
            boolean refused = false;
            try {
                service.connectToNode(impostor);
            } catch (ConnectTransportException e) {
                refused = true;
            }
            check(refused, "java refuses the rust node under another ephemeral id");
        } finally {
            service.close();
        }
    }

    public static void main(String[] args) throws Exception {
        final ThreadPool pool = new ThreadPool(Settings.builder().put("node.name", "java1").build());
        try {
            if (args[0].equals("serve")) {
                final TransportService service = service(pool, false);
                final TransportAddress address = service.getLocalNode().getAddress();
                System.out.println(address.getPort() + " " + service.getLocalNode().getEphemeralId());
                System.out.flush();
                // Serve until stdin closes.
                while (System.in.read() != -1) {
                }
                service.close();
            } else {
                call(pool, Integer.parseInt(args[1]), args[2], false);
                call(pool, Integer.parseInt(args[1]), args[2], true);
            }
        } finally {
            ThreadPool.terminate(pool, 10, TimeUnit.SECONDS);
        }
    }
}
