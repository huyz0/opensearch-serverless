/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * The OpenSearch Contributors require contributions made to
 * this file be licensed under the Apache-2.0 license or a
 * compatible open source license.
 */

package org.opensearch.serverless.snapshot;

import org.opensearch.common.blobstore.BlobPath;
import org.opensearch.common.blobstore.fs.FsBlobStore;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;
import org.opensearch.serverless.metadata.MetadataPlane;
import org.opensearch.serverless.repository.SnapshotRecord;
import org.opensearch.test.OpenSearchTestCase;

import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

/** A Java node reads the global state a Rust take wrote (fixtures/snapshots/rust-take-global/state.bin). */
public class ServerlessRustGlobalStateTests extends OpenSearchTestCase {

    private SnapshotRecord record() throws Exception {
        final String dir = System.getProperty("tests.rust.global");
        assumeTrue("set -Dtests.rust.global to fixtures/snapshots/rust-take-global", dir != null);
        try (var input = new BytesArray(Files.readAllBytes(Path.of(dir).resolve("state.bin"))).streamInput()) {
            input.readString();
            final SnapshotRecord record = SnapshotRecord.fromStream(input.readBytesReference().streamInput());
            assertTrue(input.readBoolean());
            assertEquals(0, input.available());
            return record;
        }
    }

    public void testTheStateIsReadStrictlyWithEverySetAndSetting() throws Exception {
        final var parsed = SnapshotGlobalState.read(record().globalState());
        assertEquals(4, parsed.settings().size());
        assertEquals("a b", parsed.settings().get("cluster.text"));
        assertEquals("true", String.valueOf(parsed.settings().get("cluster.flag")));
        assertEquals("7", String.valueOf(parsed.settings().get("cluster.int")));
        assertEquals("1.5", String.valueOf(parsed.settings().get("cluster.ratio")));
        assertEquals(5, parsed.sets().size());
        for (String set : new String[] {
            "index_templates",
            "component_templates",
            "ingest_pipelines",
            "search_pipelines",
            "stored_scripts" }) {
            assertEquals(set, 1, parsed.sets().get(set).size());
        }
        final Map<String, Object> pipeline = parsed.sets().get("ingest_pipelines").get("ip");
        assertTrue(pipeline.toString(), pipeline.toString().contains("é \"q\""));
    }

    public void testTheRecordKeepsTheStateThroughAJavaRoundTrip() throws Exception {
        final SnapshotRecord record = record();
        final SnapshotRecord again = SnapshotRecord.fromStream(new ByteArrayInputStream(BytesReference.toBytes(record.toBytes())));
        assertEquals(record.globalState(), again.globalState());
        SnapshotGlobalState.checkRecordFits(record);
    }

    public void testTheStateIsWithinTheBoundsOfAnEmptyPlane() throws Exception {
        final MetadataPlane plane = new MetadataPlane(new FsBlobStore(1024, createTempDir(), false), BlobPath.cleanPath(), () -> 1_000L, 300_000L);
        SnapshotGlobalState.checkBounds(plane, SnapshotGlobalState.read(record().globalState()));
    }
}
