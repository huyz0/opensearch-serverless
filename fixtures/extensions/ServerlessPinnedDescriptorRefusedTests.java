/*
 * SPDX-License-Identifier: Apache-2.0
 */

package org.opensearch.serverless.testkit;

import org.opensearch.cluster.metadata.IndexMetadata;
import org.opensearch.common.settings.Settings;
import org.opensearch.index.analysis.AnalysisTestsHelper;
import org.opensearch.serverless.cluster.IndexDescriptor;
import org.opensearch.test.OpenSearchTestCase;

import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

/**
 * A Java node reads the descriptor a Rust create wrote with an analyzer pin
 * (fixtures/extensions/pinned-descriptor.bin in opensearch-serverless, ADR 0053) and must refuse it:
 * the pin's type {@code serverless_module} is not an analyzer type the Java has, so building the index's
 * analyzers fails. Runs only when {@code -Dtests.rust.pinned=<file>} names the descriptor.
 */
public class ServerlessPinnedDescriptorRefusedTests extends OpenSearchTestCase {

    public void testAPinnedDescriptorIsRefusedWhenItsAnalyzersAreBuilt() throws Exception {
        final String file = System.getProperty("tests.rust.pinned");
        assumeTrue("needs -Dtests.rust.pinned", file != null && file.isEmpty() == false);
        final IndexDescriptor descriptor = IndexDescriptor.fromStream(new ByteArrayInputStream(Files.readAllBytes(Path.of(file))));
        final IndexMetadata metadata = descriptor.toIndexMetadata(Map.of());
        final Settings settings = Settings.builder().put(metadata.getSettings()).put("path.home", createTempDir()).build();
        assertEquals(
            "the pin reaches the settings a node builds analyzers from",
            "serverless_module",
            settings.get("index.analysis.analyzer.ext_stem.type")
        );
        final IllegalArgumentException refused = expectThrows(
            IllegalArgumentException.class,
            () -> AnalysisTestsHelper.createTestAnalysisFromSettings(settings)
        );
        assertTrue(refused.getMessage(), refused.getMessage().contains("Unknown analyzer type [serverless_module] for [ext_stem]"));
        logger.info("PASS: a Java node refused a Rust pin: {}", refused.getMessage());
    }
}
