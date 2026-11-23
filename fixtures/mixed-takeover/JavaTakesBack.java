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
import org.opensearch.serverless.metadata.Acquisition;
import org.opensearch.serverless.metadata.ShardHead;
import org.opensearch.serverless.metadata.ShardHeadStore;

import org.opensearch.common.blobstore.BlobContainer;
import org.opensearch.common.blobstore.BlobMetadata;
import org.opensearch.common.blobstore.BlobPath;
import org.opensearch.common.blobstore.BlobRegister;
import org.opensearch.common.blobstore.BlobRegisterCasResult;
import org.opensearch.common.blobstore.ConditionalBlobContainer;
import org.opensearch.common.blobstore.DeleteResult;
import org.opensearch.core.common.bytes.BytesArray;
import org.opensearch.core.common.bytes.BytesReference;

import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.util.List;
import java.util.Optional;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.Map;

/**
 * A Java node meets a shard a Rust writer took from the shell and published to: it opens the Rust node's
 * commit the way the shell does, reading the files the commit still keeps in the Java node pack and segment blob as well as
 * the Rust node's own, and then takes the shard back through the shell's {@link ShardHeadStore} once the Rust node is dead.
 * Prints what it finds.
 */
public final class JavaTakesBack {

    private static final long GENERATION_BYTES = 8L;

    public static void main(String[] args) throws Exception {
        final Path dir = Path.of(args[0]);
        final String index = "fixture";
        final String uuid = "fixture-uuid-0000000";
        final Path shard = dir.resolve("segments/" + index + "#" + uuid + "#0");

        final byte[] register = Files.readAllBytes(shard.resolve("manifest"));
        final CommitManifest manifest = CommitManifest.fromStream(
            new ByteArrayInputStream(Arrays.copyOfRange(register, (int) GENERATION_BYTES, register.length))
        );
        System.out.println("term " + manifest.term() + " writer " + manifest.writer() + " files " + manifest.files().size());
        long inPack = 0;
        long inBlob = 0;
        try (Directory target = new ByteBuffersDirectory()) {
            for (Map.Entry<String, String> file : manifest.files().entrySet()) {
                final FileLocation location = FileLocation.parse(file.getKey(), file.getValue());
                final Path blob;
                if (location.termDir().startsWith("@")) {
                    blob = dir.resolve("packs").resolve(location.termDir().substring(1)).resolve(location.blob());
                    inPack++;
                } else {
                    blob = shard.resolve(location.termDir()).resolve(location.blob());
                    inBlob++;
                }
                final byte[] bytes = new byte[Math.toIntExact(location.length() < 0 ? Files.size(blob) : location.length())];
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
            System.out.println("files_in_node_pack " + inPack);
            System.out.println("files_in_blobs " + inBlob);
            try (DirectoryReader reader = new SoftDeletesDirectoryReaderWrapper(DirectoryReader.open(target), "__soft_deletes")) {
                System.out.println("num_docs " + reader.numDocs());
                System.out.println("max_doc " + reader.maxDoc());
            }
        }

        final ShardHeadStore heads = new ShardHeadStore(new DirRegisters(dir.resolve("shards")), () -> 4_102_444_800_000L, 30_000L, (node, ephemeral) -> false);
        final ShardHead before = heads.read(index, 0).orElseThrow();
        System.out.println("head_before term " + before.term() + " owner " + before.ownerNodeId() + " writers " + before.writers());
        final Acquisition taken = heads.acquire(index, 0, "java-1", "java-eph", uuid);
        System.out.println("acquired " + taken.acquired() + " term " + taken.head().term() + " owner " + taken.head().ownerNodeId()
            + " previous " + taken.previousOwner() + " writers " + taken.head().writers());
    }
}

/** The registers of a directory as the filesystem store keeps them: an 8-byte generation, then the value. */
final class DirRegisters implements ConditionalBlobContainer {
    private final Path root;

    DirRegisters(Path root) {
        this.root = root;
    }

    @Override
    public Optional<BlobRegister> readRegister(String blobName) throws IOException {
        final Path file = root.resolve(blobName);
        return Files.exists(file) ? Optional.of(BlobRegister.fromBytes(Files.readAllBytes(file))) : Optional.empty();
    }

    @Override
    public BlobRegisterCasResult compareAndSwapRegister(String blobName, long expected, BytesReference value) throws IOException {
        final Optional<BlobRegister> current = readRegister(blobName);
        final long actual = current.map(BlobRegister::generation).orElse(BlobRegister.ABSENT_GENERATION);
        if (actual != expected) {
            return BlobRegisterCasResult.conflict(actual);
        }
        final long next = current.isEmpty() ? BlobRegister.initialGeneration() : actual + 1;
        final Path file = root.resolve(blobName);
        Files.createDirectories(file.getParent());
        Files.write(file, BlobRegister.toBytes(next, value));
        return BlobRegisterCasResult.applied(next);
    }

    @Override
    public BlobPath path() {
        throw new UnsupportedOperationException();
    }

    @Override
    public boolean blobExists(String blobName) {
        return Files.exists(root.resolve(blobName));
    }

    @Override
    public InputStream readBlob(String blobName) {
        throw new UnsupportedOperationException();
    }

    @Override
    public InputStream readBlob(String blobName, long position, long length) {
        throw new UnsupportedOperationException();
    }

    @Override
    public void writeBlob(String blobName, InputStream in, long size, boolean failIfAlreadyExists) {
        throw new UnsupportedOperationException();
    }

    @Override
    public void writeBlobAtomic(String blobName, InputStream in, long size, boolean failIfAlreadyExists) {
        throw new UnsupportedOperationException();
    }

    @Override
    public DeleteResult delete() {
        throw new UnsupportedOperationException();
    }

    @Override
    public void deleteBlobsIgnoringIfNotExists(List<String> blobNames) {
        throw new UnsupportedOperationException();
    }

    @Override
    public java.util.Map<String, BlobMetadata> listBlobs() {
        throw new UnsupportedOperationException();
    }

    @Override
    public java.util.Map<String, BlobContainer> children() {
        throw new UnsupportedOperationException();
    }

    @Override
    public java.util.Map<String, BlobMetadata> listBlobsByPrefix(String prefix) {
        throw new UnsupportedOperationException();
    }
}
