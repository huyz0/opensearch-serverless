/*
 * SPDX-License-Identifier: Apache-2.0
 */

package org.opensearch.serverless.testkit;

import org.opensearch.common.blobstore.BlobPath;
import org.opensearch.common.blobstore.fs.FsBlobStore;
import org.opensearch.common.settings.Settings;
import org.opensearch.serverless.metadata.MetadataPlane;
import org.opensearch.serverless.shell.ServerlessNode;
import org.opensearch.test.OpenSearchTestCase;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicLong;
import java.util.stream.Stream;

/**
 * A Java node restores a shallow snapshot a Rust node took, over the directory the Rust node wrote
 * ({@code OSS_WRITE_BUCKET}, see fixtures/snapshots/README.md in opensearch-serverless), and checks the index it creates.
 * Opening the restored shard on a Java writer is not part of this: see fixtures/snapshots/README.md. Runs only when {@code -Dtests.rust.bucket=<dir>} names that directory; the directory is copied first.
 */
public class ServerlessRustTakeRestoreTests extends OpenSearchTestCase {

    public void testARustTakeIsRestoredAndSearchedByAJavaNode() throws Exception {
        final String bucket = System.getProperty("tests.rust.bucket");
        assumeTrue("needs -Dtests.rust.bucket", bucket != null && bucket.isEmpty() == false);
        final Path root = createTempDir();
        copy(Path.of(bucket), root);
        final AtomicLong clock = new AtomicLong(1_700_000_000_000L);
        final MetadataPlane plane = new MetadataPlane(new FsBlobStore(1024, root, false), BlobPath.cleanPath(), clock::get, 300_000L);
        final Settings settings = Settings.builder()
            .put("node.name", "rust-take")
            .put("cluster.name", "serverless-rust-take")
            .put("path.home", createTempDir())
            .put("network.host", "127.0.0.1")
            .put("http.port", "0")
            .put("transport.port", "0")
            .put("serverless.roles", "ingest")
            .build();
        try (ServerlessNode node = new ServerlessNode(settings)) {
            node.startup().start();
            node.setMetadataPlane(plane);

            final String listed = send(node, "GET", "/_snapshot/backups/daily", null);
            assertTrue(listed, listed.contains("\"state\":\"SUCCESS\"") || listed.contains("daily"));

            final String restored = send(
                node,
                "POST",
                "/_snapshot/backups/daily/_restore?wait_for_completion=true",
                "{\"rename_pattern\":\"logs\",\"rename_replacement\":\"copy\"}"
            );
            assertTrue(restored, restored.contains("\"indices\":[\"copy\"]"));

            assertTrue("the restored index is described", plane.describe("copy").isPresent());
            assertEquals("the restored copy keeps its three shards' worth: one shard", 1, plane.describe("copy").get().numberOfShards());
            logger.info("PASS: Java restored a Rust take: {}", restored);
        }
    }

    private static void copy(Path from, Path to) throws IOException {
        try (Stream<Path> walk = Files.walk(from)) {
            for (Path source : (Iterable<Path>) walk::iterator) {
                final Path target = to.resolve(from.relativize(source).toString());
                if (Files.isDirectory(source)) {
                    Files.createDirectories(target);
                } else {
                    Files.copy(source, target);
                }
            }
        }
    }

    private static String send(ServerlessNode node, String method, String path, String body) throws Exception {
        final var address = node.boundHttpAddress().publishAddress();
        try (HttpClient client = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(10)).build()) {
            final HttpRequest request = HttpRequest.newBuilder()
                .uri(URI.create("http://" + address.getAddress() + ":" + address.getPort() + path))
                .timeout(Duration.ofSeconds(60))
                .header("Content-Type", "application/json")
                .method(
                    method,
                    body == null ? HttpRequest.BodyPublishers.noBody() : HttpRequest.BodyPublishers.ofString(body, StandardCharsets.UTF_8)
                )
                .build();
            final HttpResponse<String> response = client.send(request, HttpResponse.BodyHandlers.ofString());
            return response.statusCode() + " " + response.body();
        }
    }
}
