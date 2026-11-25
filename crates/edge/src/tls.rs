//! The TLS server settings: a certificate chain and key from PEM files, ALPN `h2` then `http/1.1`.

use std::path::Path;
use std::sync::Arc;

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;

/// Builds the acceptor. TLS 1.2 and 1.3 only; the key is read once and never logged.
pub(crate) fn acceptor(cert: &Path, key: &Path) -> Result<TlsAcceptor, String> {
    let chain = CertificateDer::pem_file_iter(cert)
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        .map_err(|e| format!("edge certificate {}: {e}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        .map_err(|e| format!("edge certificate {}: {e}", cert.display()))?;
    if chain.is_empty() {
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        return Err(format!(
            "edge certificate {} holds no certificate",
            cert.display()
        ));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        .map_err(|e| format!("edge key {}: {e}", key.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        .map_err(|e| format!("edge tls versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        // ALLOC: start-time certificate parse, once, bounded by the PEM file.
        .map_err(|e| format!("edge certificate and key do not match or are unusable: {e}"))?;
    // ALLOC: start-time certificate parse, once, bounded by the PEM file.
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}
