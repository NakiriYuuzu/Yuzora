//! rustls client configs for PostgreSQL.
//!
//! Always the ring provider, passed to every config; nothing is installed as
//! the process default (the app has other TLS users). The modes follow libpq:
//!
//! | mode          | encrypts            | certificate chain | host name |
//! |---------------|---------------------|-------------------|-----------|
//! | `disable`     | never               |                   |           |
//! | `prefer`      | if the server can   | not checked       | no        |
//! | `require`     | always              | not checked       | no        |
//! | `verify-ca`   | always              | checked           | no        |
//! | `verify-full` | always              | checked           | checked   |
//!
//! `verify-full` trusts the Mozilla roots plus the PEM file of `ca_file`.
//! `verify-ca` trusts `ca_file` alone and needs one: with the public roots and
//! no name check, any certificate a public CA issued for any domain would
//! pass. Handshake signatures are verified in every mode: a certificate that
//! is not checked still has to prove it owns its key.

use std::path::PathBuf;
use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use rustls_pki_types::pem::PemObject;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::config::SslMode;
use crate::error::{DbError, DbResult};

/// How the client asks tokio-postgres to negotiate.
pub(super) fn pg_mode(ssl: SslMode) -> tokio_postgres::config::SslMode {
    match ssl {
        SslMode::Disable => tokio_postgres::config::SslMode::Disable,
        SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
    }
}

/// The connector for `ssl`, with `ca_file` (PEM, may hold several
/// certificates; `~` is home) among the roots of the `verify-*` modes.
/// `disable` still gets one (never used): the cancel path wants the same
/// type either way.
pub(super) fn connector(ssl: SslMode, ca_file: &str) -> DbResult<MakeRustlsConnect> {
    client_config(ssl, ca_file).map(MakeRustlsConnect::new)
}

fn client_config(ssl: SslMode, ca_file: &str) -> DbResult<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::Tls(e.to_string()))?;
    Ok(match ssl {
        SslMode::Disable | SslMode::Prefer | SslMode::Require => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
            .with_no_client_auth(),
        SslMode::VerifyFull => builder.with_root_certificates(roots(ca_file, true)?).with_no_client_auth(),
        SslMode::VerifyCa => {
            if ca_file.trim().is_empty() {
                return Err(DbError::Tls(crate::i18n::t("engine.ca_required", &[])));
            }
            let inner = WebPkiServerVerifier::builder_with_provider(Arc::new(roots(ca_file, false)?), provider)
                .build()
                .map_err(|e| DbError::Tls(e.to_string()))?;
            builder.dangerous().with_custom_certificate_verifier(Arc::new(ChainOnly(inner))).with_no_client_auth()
        }
    })
}

/// The trusted roots: every certificate in `ca_file`, and Mozilla's when `public`.
fn roots(ca_file: &str, public: bool) -> DbResult<RootCertStore> {
    let mut roots = RootCertStore::empty();
    if public {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    let path = ca_file.trim();
    if path.is_empty() {
        return Ok(roots);
    }
    let path = expand_home(path);
    let pem =
        std::fs::read(&path).map_err(|e| DbError::Tls(crate::i18n::t("engine.ca_unreadable", &[("path", path.display().to_string()), ("error", e.to_string())])))?;
    let mut added = 0;
    for cert in CertificateDer::pem_slice_iter(&pem) {
        let cert = cert.map_err(|e| DbError::Tls(crate::i18n::t("engine.ca_not_pem", &[("path", path.display().to_string()), ("error", e.to_string())])))?;
        roots.add(cert).map_err(|e| DbError::Tls(crate::i18n::t("engine.ca_unusable", &[("path", path.display().to_string()), ("error", e.to_string())])))?;
        added += 1;
    }
    if added == 0 {
        return Err(DbError::Tls(crate::i18n::t("engine.ca_empty", &[("path", path.display().to_string())])));
    }
    Ok(roots)
}

/// `~` and `~/…` are the home folder.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/").or_else(|| (path == "~").then_some("")) {
        Some(rest) => yuzora_core::paths::home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

/// `prefer` and `require`: any certificate is accepted, but it has to sign
/// the handshake with the key it presents.
#[derive(Debug)]
struct AnyCertificate(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// `verify-ca`: the chain must lead to a root; the name on the certificate
/// does not matter. The name is checked after the chain, so a name mismatch
/// is only ever reported for a chain that was fine.
#[derive(Debug)]
struct ChainOnly(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match self.0.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_onto_the_negotiation_tokio_postgres_does() {
        use tokio_postgres::config::SslMode as Pg;
        assert_eq!(pg_mode(SslMode::Disable), Pg::Disable);
        assert_eq!(pg_mode(SslMode::Prefer), Pg::Prefer);
        for strict in [SslMode::Require, SslMode::VerifyCa, SslMode::VerifyFull] {
            assert_eq!(pg_mode(strict), Pg::Require);
        }
    }

    #[test]
    fn every_mode_builds_a_connector() {
        for ssl in [SslMode::Disable, SslMode::Prefer, SslMode::Require, SslMode::VerifyFull] {
            assert!(connector(ssl, "").is_ok(), "{ssl:?}");
        }
        // Public roots without a name check would let any public certificate through.
        let err = connector(SslMode::VerifyCa, "").err().unwrap();
        assert_eq!(err, DbError::Tls(crate::i18n::t("engine.ca_required", &[])));
    }

    #[test]
    fn the_ca_file_is_read_as_pem() {
        let dir = std::env::temp_dir().join(format!("yuzora-db-tls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.pem");
        let err = connector(SslMode::VerifyFull, missing.to_str().unwrap()).err().unwrap();
        assert!(matches!(&err, DbError::Tls(m) if m.contains("missing.pem")), "{err:?}");

        let empty = dir.join("empty.pem");
        std::fs::write(&empty, "nothing here\n").unwrap();
        let err = connector(SslMode::VerifyCa, empty.to_str().unwrap()).err().unwrap();
        let none = crate::i18n::t("engine.ca_empty", &[("path", empty.display().to_string())]);
        assert!(matches!(&err, DbError::Tls(m) if m == &none), "{err:?}");

        // The modes that don't verify never read the file.
        assert!(connector(SslMode::Require, missing.to_str().unwrap()).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The MSSQL fixture's CA and the server certificate it signed (not a CA
    /// itself; it names `localhost` and `127.0.0.1`).
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/database/fixtures/mssql/tls").join(name)
    }

    /// A handshake between a client of `ssl` and a server holding the fixture
    /// certificate, in memory; the client asks for `name`.
    fn handshake(ssl: SslMode, ca: Option<&str>, name: &str) -> Result<(), Error> {
        use rustls::pki_types::PrivateKeyDer;
        let ca = ca.map(|n| fixture(n).to_string_lossy().into_owned()).unwrap_or_default();
        let client_config = client_config(ssl, &ca).expect("client config");
        let chain: Vec<_> = CertificateDer::pem_slice_iter(&std::fs::read(fixture("server.pem")).unwrap())
            .collect::<Result<_, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(&std::fs::read(fixture("server.key")).unwrap()).unwrap();
        let server_config =
            rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(chain, key)
                .unwrap();
        let mut client =
            rustls::ClientConnection::new(Arc::new(client_config), ServerName::try_from(name.to_owned()).unwrap())
                .unwrap();
        let mut server = rustls::ServerConnection::new(Arc::new(server_config)).unwrap();
        while client.is_handshaking() || server.is_handshaking() {
            let mut moved = false;
            while client.wants_write() {
                let mut bytes = Vec::new();
                client.write_tls(&mut bytes).unwrap();
                server.read_tls(&mut bytes.as_slice()).unwrap();
                server.process_new_packets()?;
                moved = true;
            }
            while server.wants_write() {
                let mut bytes = Vec::new();
                server.write_tls(&mut bytes).unwrap();
                client.read_tls(&mut bytes.as_slice()).unwrap();
                client.process_new_packets()?;
                moved = true;
            }
            assert!(moved, "the handshake stalled");
        }
        Ok(())
    }

    fn unknown_issuer(result: Result<(), Error>) -> bool {
        matches!(result, Err(Error::InvalidCertificate(CertificateError::UnknownIssuer)))
    }

    fn wrong_name(result: Result<(), Error>) -> bool {
        matches!(
            result,
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
            ))
        )
    }

    #[test]
    fn verify_full_wants_a_known_issuer_and_the_right_name() {
        let ca = Some("ca.pem");
        assert_eq!(handshake(SslMode::VerifyFull, ca, "localhost"), Ok(()));
        assert_eq!(handshake(SslMode::VerifyFull, ca, "127.0.0.1"), Ok(()));
        assert!(wrong_name(handshake(SslMode::VerifyFull, ca, "db.invalid")));
        assert!(unknown_issuer(handshake(SslMode::VerifyFull, None, "localhost")));
        assert!(unknown_issuer(handshake(SslMode::VerifyFull, Some("other-ca.pem"), "localhost")));
    }

    #[test]
    fn verify_ca_wants_a_known_issuer_and_nothing_about_the_name() {
        let ca = Some("ca.pem");
        assert_eq!(handshake(SslMode::VerifyCa, ca, "localhost"), Ok(()));
        assert_eq!(handshake(SslMode::VerifyCa, ca, "db.invalid"), Ok(()));
        assert!(unknown_issuer(handshake(SslMode::VerifyCa, Some("other-ca.pem"), "db.invalid")));
        // Without a CA there is nothing to trust (every_mode_builds_a_connector).
    }

    #[test]
    fn prefer_and_require_encrypt_without_checking_who_answers() {
        for ssl in [SslMode::Prefer, SslMode::Require] {
            assert_eq!(handshake(ssl, None, "db.invalid"), Ok(()), "{ssl:?}");
        }
    }

    #[test]
    fn home_is_expanded() {
        assert_eq!(expand_home("/etc/ca.pem"), PathBuf::from("/etc/ca.pem"));
        assert_eq!(expand_home("~/ca.pem"), yuzora_core::paths::home_dir().join("ca.pem"));
    }
}
