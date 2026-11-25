//! `StoreChoice`'s `Debug`, which never prints the secret.

use std::fmt;

use super::StoreChoice;

impl fmt::Debug for StoreChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fs { root } => f.debug_struct("Fs").field("root", root).finish(),
            Self::S3 {
                endpoint,
                bucket,
                region,
                access_key,
                ..
            } => f
                .debug_struct("S3")
                .field("endpoint", endpoint)
                .field("bucket", bucket)
                .field("region", region)
                .field("access_key", access_key)
                .field("secret_key", &"<redacted>")
                .finish(),
        }
    }
}
