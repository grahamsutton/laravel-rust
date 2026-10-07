//! TLS configuration for the transport (rustls, with the ring backend).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use illuminate_support::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{
    CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::exceptions::ConnectionException;

/// How the server's certificate should be verified.
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub(crate) enum Verify {
    /// Verify against the bundled Mozilla root certificates.
    #[default]
    Default,
    /// Verify against the certificates in the given PEM bundle.
    Bundle(PathBuf),
    /// Don't verify the certificate at all (dangerous!).
    Disabled,
}

fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The client configuration for the given verification mode.
pub(crate) fn client_config(verify: &Verify) -> Result<Arc<ClientConfig>> {
    static DEFAULT: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static INSECURE: OnceLock<Arc<ClientConfig>> = OnceLock::new();

    match verify {
        Verify::Default => {
            if let Some(config) = DEFAULT.get() {
                return Ok(config.clone());
            }
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            Ok(DEFAULT.get_or_init(|| build(roots)).clone())
        }
        Verify::Disabled => Ok(INSECURE
            .get_or_init(|| {
                let mut config = build(RootCertStore::empty());
                Arc::make_mut(&mut config)
                    .dangerous()
                    .set_certificate_verifier(Arc::new(NoVerification(
                        provider().signature_verification_algorithms,
                    )));
                config
            })
            .clone()),
        Verify::Bundle(path) => Ok(build(load_bundle(path)?)),
    }
}

fn build(roots: RootCertStore) -> Arc<ClientConfig> {
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();

    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

fn load_bundle(path: &Path) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();

    let certificates = CertificateDer::pem_file_iter(path).map_err(|error| {
        ConnectionException::new(format!(
            "Unable to read the certificate bundle [{}]: {error}",
            path.display()
        ))
    })?;

    for certificate in certificates {
        let certificate = certificate.map_err(|error| {
            ConnectionException::new(format!(
                "Invalid certificate in [{}]: {error}",
                path.display()
            ))
        })?;
        roots.add(certificate).map_err(|error| {
            ConnectionException::new(format!(
                "Invalid certificate in [{}]: {error}",
                path.display()
            ))
        })?;
    }

    Ok(roots)
}

/// The server name to present (SNI) and verify for the given host.
pub(crate) fn server_name(host: &str) -> Result<ServerName<'static>> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(host.to_string())
        .map_err(|_| ConnectionException::new(format!("Invalid TLS server name [{host}].")).into())
}

/// A certificate "verifier" that accepts any certificate, used by
/// `without_verifying`. Handshake signatures are still checked, so the
/// connection is encrypted; the server's identity simply isn't proven.
#[derive(Debug)]
struct NoVerification(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configurations_are_cached_and_speak_http_1_1() {
        let default = client_config(&Verify::Default).unwrap();
        assert!(Arc::ptr_eq(
            &default,
            &client_config(&Verify::Default).unwrap()
        ));
        assert_eq!(default.alpn_protocols, vec![b"http/1.1".to_vec()]);

        let insecure = client_config(&Verify::Disabled).unwrap();
        assert!(!Arc::ptr_eq(&default, &insecure));
    }

    #[test]
    fn missing_bundles_are_connection_errors() {
        let error = client_config(&Verify::Bundle("/definitely/missing.pem".into())).unwrap_err();
        assert!(error.is::<ConnectionException>());
        assert!(error.to_string().contains("/definitely/missing.pem"));
    }

    #[test]
    fn server_names_accept_domains_and_ips() {
        assert!(server_name("laravel.com").is_ok());
        assert!(server_name("127.0.0.1").is_ok());
        assert!(server_name("[::1]").is_ok());
        assert!(server_name("not a host").is_err());
    }
}
