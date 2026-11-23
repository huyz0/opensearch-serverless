package org.opensearch.common.blobstore;

import java.io.IOException;
import java.io.InputStream;
import java.util.List;
import java.util.Map;

/** Recording stub: the methods of the server's container that the recorder's directory implements. */
public interface BlobContainer {
    BlobPath path();

    boolean blobExists(String blobName) throws IOException;

    InputStream readBlob(String blobName) throws IOException;

    InputStream readBlob(String blobName, long position, long length) throws IOException;

    void writeBlob(String blobName, InputStream inputStream, long blobSize, boolean failIfAlreadyExists) throws IOException;

    void writeBlobAtomic(String blobName, InputStream inputStream, long blobSize, boolean failIfAlreadyExists) throws IOException;

    DeleteResult delete() throws IOException;

    void deleteBlobsIgnoringIfNotExists(List<String> blobNames) throws IOException;

    Map<String, BlobMetadata> listBlobs() throws IOException;

    Map<String, BlobContainer> children() throws IOException;

    Map<String, BlobMetadata> listBlobsByPrefix(String blobNamePrefix) throws IOException;
}
