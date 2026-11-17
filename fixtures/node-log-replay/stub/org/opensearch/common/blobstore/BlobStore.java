package org.opensearch.common.blobstore;

/** Just what the node log uses of the server's BlobStore. */
public interface BlobStore {
    BlobContainer blobContainer(BlobPath path);
}
