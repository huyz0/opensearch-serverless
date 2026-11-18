/*
 * SPDX-License-Identifier: Apache-2.0
 */

package org.opensearch.serverless.store;

import org.apache.lucene.index.DirectoryReader;
import org.apache.lucene.index.SoftDeletesDirectoryReaderWrapper;
import org.apache.lucene.store.ByteBuffersDirectory;
import org.apache.lucene.store.Directory;
import org.apache.lucene.store.IOContext;
import org.apache.lucene.store.IndexOutput;

import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.RandomAccessFile;
import java.nio.file.Path;
import java.util.Map;

/**
 * Opens a commit the Rust writer published the way the shell does: the commit register's value through
 * {@link CommitManifest#fromStream}, each file's blob range through {@link FileLocation#parse}, into a
 * directory a Lucene {@link DirectoryReader} opens through the soft-deletes wrapper. Prints what it counts.
 */
public final class JavaOpensRust {

    public static void main(String[] args) throws IOException {
        final Path dir = Path.of(args[0]);
        final Path shard = dir.resolve("store/segments/logs#u1#0");
        final CommitManifest manifest;
        try (InputStream in = new FileInputStream(dir.resolve("manifest.bin").toFile())) {
            manifest = CommitManifest.fromStream(in);
        }
        System.out.println("term " + manifest.term() + " writer " + manifest.writer() + " files " + manifest.files().size());
        try (Directory target = new ByteBuffersDirectory()) {
            for (Map.Entry<String, String> file : manifest.files().entrySet()) {
                final FileLocation location = FileLocation.parse(file.getKey(), file.getValue());
                final Path blob = shard.resolve(location.termDir()).resolve(location.blob());
                final byte[] bytes = new byte[Math.toIntExact(location.length())];
                try (RandomAccessFile raf = new RandomAccessFile(blob.toFile(), "r")) {
                    raf.seek(location.offset());
                    raf.readFully(bytes);
                }
                final Long length = manifest.lengthOf(file.getKey());
                if (length != null && length != bytes.length) {
                    throw new IOException(file.getKey() + ": manifest says " + length + ", location says " + bytes.length);
                }
                try (IndexOutput out = target.createOutput(file.getKey(), IOContext.DEFAULT)) {
                    out.writeBytes(bytes, bytes.length);
                }
            }
            try (DirectoryReader reader = new SoftDeletesDirectoryReaderWrapper(DirectoryReader.open(target), "__soft_deletes")) {
                System.out.println("num_docs " + reader.numDocs());
                System.out.println("max_doc " + reader.maxDoc());
            }
        }
    }
}
