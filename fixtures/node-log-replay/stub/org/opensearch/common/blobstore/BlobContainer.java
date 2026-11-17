package org.opensearch.common.blobstore;

import java.io.IOException;
import java.io.InputStream;
import java.util.List;

/** Just the calls the node log makes on the server's BlobContainer. */
public interface BlobContainer {
    boolean blobExists(String blobName) throws IOException;

    InputStream readBlob(String blobName) throws IOException;

    InputStream readBlob(String blobName, long position, long length) throws IOException;

    void writeBlob(String blobName, InputStream inputStream, long blobSize, boolean failIfAlreadyExists) throws IOException;

    void deleteBlobsIgnoringIfNotExists(List<String> blobNames) throws IOException;
}
