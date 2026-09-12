//! Pinned-identity TLS transport for an SMP relay.

use std::{
    io::{self, Read, Write},
    net::TcpStream,
};

use openssl::{
    ssl::{SslConnector, SslMethod, SslStream, SslVerifyMode, SslVersion},
    stack::Stack,
    x509::{X509, X509StoreContext, store::X509StoreBuilder},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("TLS configuration failed: {0}")]
    Configuration(#[source] openssl::error::ErrorStack),
    #[error("TLS handshake failed: {0}")]
    Handshake(String),
    #[error("server certificate chain must contain 2 to 4 certificates")]
    InvalidChainLength,
    #[error("server identity certificate fingerprint does not match")]
    IdentityMismatch,
    #[error("server certificate chain validation failed")]
    InvalidChain,
    #[error("certificate encoding failed: {0}")]
    CertificateEncoding(#[source] openssl::error::ErrorStack),
    #[error("TLS channel binding is unavailable")]
    MissingChannelBinding,
}

/// A TLS stream whose peer chain has been validated against the pinned SMP
/// identity. No application data is sent before validation succeeds.
pub struct SmpTlsStream {
    stream: SslStream<TcpStream>,
    peer_certificates: Vec<Vec<u8>>,
    tls_unique: Vec<u8>,
}

impl SmpTlsStream {
    pub fn peer_certificates(&self) -> &[Vec<u8>] {
        &self.peer_certificates
    }

    pub fn tls_unique(&self) -> &[u8] {
        &self.tls_unique
    }

    pub fn alpn(&self) -> Option<&[u8]> {
        self.stream.ssl().selected_alpn_protocol()
    }
}

impl Read for SmpTlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf)
    }
}

impl Write for SmpTlsStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// Complete a TLS handshake on an already connected TCP socket. The caller
/// supplies the SHA-256 fingerprint from the SimpleX server address.
pub fn connect_smp_tls(
    tcp: TcpStream,
    server_name: &str,
    expected_fingerprint: &[u8; 32],
) -> Result<SmpTlsStream, TlsError> {
    let mut builder =
        SslConnector::builder(SslMethod::tls_client()).map_err(TlsError::Configuration)?;
    builder
        .set_min_proto_version(Some(SslVersion::TLS1_2))
        .map_err(TlsError::Configuration)?;
    builder
        .set_alpn_protos(b"\x05smp/1")
        .map_err(TlsError::Configuration)?;
    // The relay's CA is pinned by identity rather than installed in the OS
    // trust store. Validate its chain immediately after TLS negotiation and
    // before the stream is returned or application data is exchanged.
    builder.set_verify(SslVerifyMode::NONE);
    let connector = builder.build();
    let stream = connector
        .connect(server_name, tcp)
        .map_err(|error| TlsError::Handshake(error.to_string()))?;
    let chain = stream
        .ssl()
        .peer_cert_chain()
        .ok_or(TlsError::InvalidChainLength)?;
    let certificates: Vec<X509> = chain.iter().map(|cert| cert.to_owned()).collect();
    validate_pinned_chain(&certificates, expected_fingerprint)?;
    let peer_certificates = certificates
        .iter()
        .map(|cert| cert.to_der().map_err(TlsError::CertificateEncoding))
        .collect::<Result<Vec<_>, _>>()?;
    let mut finished = [0u8; 64];
    let length = stream.ssl().finished(&mut finished);
    if length == 0 {
        return Err(TlsError::MissingChannelBinding);
    }
    Ok(SmpTlsStream {
        stream,
        peer_certificates,
        tls_unique: finished[..length].to_vec(),
    })
}

fn validate_pinned_chain(
    certificates: &[X509],
    expected_fingerprint: &[u8; 32],
) -> Result<(), TlsError> {
    if !(2..=4).contains(&certificates.len()) {
        return Err(TlsError::InvalidChainLength);
    }
    let identity_der = certificates[1]
        .to_der()
        .map_err(TlsError::CertificateEncoding)?;
    if Sha256::digest(&identity_der).as_slice() != expected_fingerprint {
        return Err(TlsError::IdentityMismatch);
    }
    let mut store = X509StoreBuilder::new().map_err(TlsError::Configuration)?;
    store
        .add_cert(certificates.last().unwrap().clone())
        .map_err(TlsError::Configuration)?;
    let store = store.build();
    let mut untrusted = Stack::new().map_err(TlsError::Configuration)?;
    for certificate in &certificates[1..certificates.len() - 1] {
        untrusted
            .push(certificate.clone())
            .map_err(TlsError::Configuration)?;
    }
    let mut context = X509StoreContext::new().map_err(TlsError::Configuration)?;
    let valid = context
        .init(&store, &certificates[0], &untrusted, |ctx| {
            ctx.verify_cert()
        })
        .map_err(TlsError::Configuration)?;
    if !valid {
        return Err(TlsError::InvalidChain);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use openssl::{
        asn1::{Asn1Integer, Asn1Time},
        bn::BigNum,
        hash::MessageDigest,
        pkey::{PKey, Private},
        ssl::SslAcceptor,
        x509::{
            X509NameBuilder,
            extension::{BasicConstraints, KeyUsage},
        },
    };
    use std::{net::TcpListener, thread};

    pub(crate) fn certificate(
        serial: u32,
        subject: &str,
        key: &PKey<Private>,
        issuer: Option<(&X509, &PKey<Private>)>,
    ) -> X509 {
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", subject).unwrap();
        let name = name.build();
        let mut builder = X509::builder().unwrap();
        let serial = Asn1Integer::from_bn(&BigNum::from_u32(serial).unwrap()).unwrap();
        builder.set_serial_number(&serial).unwrap();
        builder.set_version(2).unwrap();
        builder.set_subject_name(&name).unwrap();
        builder
            .set_issuer_name(issuer.map_or(name.as_ref(), |(cert, _)| cert.subject_name()))
            .unwrap();
        builder.set_pubkey(key).unwrap();
        builder
            .set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        builder
            .set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        if issuer.is_none() {
            builder
                .append_extension(BasicConstraints::new().critical().ca().build().unwrap())
                .unwrap();
            builder
                .append_extension(KeyUsage::new().critical().key_cert_sign().build().unwrap())
                .unwrap();
        }
        builder
            .sign(
                issuer.map_or(key, |(_, issuer_key)| issuer_key),
                MessageDigest::null(),
            )
            .unwrap();
        builder.build()
    }

    #[test]
    fn validates_pinned_identity_and_certificate_chain() {
        let identity_key = PKey::generate_ed25519().unwrap();
        let identity = certificate(1, "identity", &identity_key, None);
        let leaf_key = PKey::generate_ed25519().unwrap();
        let leaf = certificate(2, "relay", &leaf_key, Some((&identity, &identity_key)));
        let fingerprint: [u8; 32] = Sha256::digest(identity.to_der().unwrap()).into();
        assert!(validate_pinned_chain(&[leaf.clone(), identity.clone()], &fingerprint).is_ok());
        assert!(matches!(
            validate_pinned_chain(&[leaf.clone(), identity.clone()], &[0; 32]),
            Err(TlsError::IdentityMismatch)
        ));
        assert!(matches!(
            validate_pinned_chain(&[leaf], &fingerprint),
            Err(TlsError::InvalidChainLength)
        ));
    }

    #[test]
    fn connects_to_local_relay_with_pinned_identity() {
        let identity_key = PKey::generate_ed25519().unwrap();
        let identity = certificate(1, "identity", &identity_key, None);
        let leaf_key = PKey::generate_ed25519().unwrap();
        let leaf = certificate(2, "localhost", &leaf_key, Some((&identity, &identity_key)));
        let fingerprint: [u8; 32] = Sha256::digest(identity.to_der().unwrap()).into();
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server()).unwrap();
        acceptor.set_certificate(&leaf).unwrap();
        acceptor.set_private_key(&leaf_key).unwrap();
        acceptor.add_extra_chain_cert(identity).unwrap();
        acceptor.check_private_key().unwrap();
        let acceptor = acceptor.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let _stream = acceptor.accept(tcp).unwrap();
        });
        let tcp = TcpStream::connect(address).unwrap();
        let stream = connect_smp_tls(tcp, "localhost", &fingerprint).unwrap();
        assert_eq!(stream.peer_certificates().len(), 2);
        assert!(!stream.tls_unique().is_empty());
        server.join().unwrap();
    }
}
