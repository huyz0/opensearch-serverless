package org.opensearch.common.blobstore;

/** Recording stub: named by the node pack store, never used here. */
public interface BlobStore {
    BlobContainer blobContainer(BlobPath path);
}
