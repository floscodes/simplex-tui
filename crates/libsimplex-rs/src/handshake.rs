//! Authenticated SMP client handshake over a pinned TLS stream.

use std::io;

use openssl::{
    derive::Deriver,
    pkey::{Id, PKey, Private, Public},
};
use thiserror::Error;

use crate::{
    auth::{AuthError, VerifiedDhKey},
    encrypted::{EncryptedBlockError, SmpBlockCipher},
    tls::SmpTlsStream,
    transport::{ServerHello, ServerHelloError, SmpVersionRange, read_smp_block, write_smp_block},
};

#[derive(Debug, Error)]
pub enum HandshakeError {
    #[error("SMP transport I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid SMP server hello: {0}")]
    ServerHello(#[from] ServerHelloError),
    #[error("SMP server authentication failed: {0}")]
    Authentication(#[from] AuthError),
    #[error("SMP key agreement failed: {0}")]
    Crypto(#[source] openssl::error::ErrorStack),
    #[error("SMP client public key is too large for the protocol")]
    PublicKeyTooLarge,
    #[error("SMP version 6 has no authenticated server key")]
    UnsupportedLegacyVersion,
    #[error("SMP block key initialization failed: {0}")]
    BlockCipher(#[from] EncryptedBlockError),
}

/// Result of the initial handshake. Further SMP traffic requires the agent
/// protocol and, for v11+, encrypted transport blocks.
pub struct SmpHandshake {
    stream: SmpTlsStream,
    pub version: u16,
    shared_secret: Vec<u8>,
    block_cipher: Option<SmpBlockCipher>,
}

impl SmpHandshake {
    pub fn shared_secret(&self) -> &[u8] {
        &self.shared_secret
    }

    pub fn send_block(&mut self, message: &[u8]) -> Result<(), HandshakeError> {
        match &mut self.block_cipher {
            Some(cipher) => cipher.write_block(&mut self.stream, message)?,
            None => write_smp_block(&mut self.stream, message)?,
        }
        Ok(())
    }

    pub fn receive_block(&mut self) -> Result<Vec<u8>, HandshakeError> {
        Ok(match &mut self.block_cipher {
            Some(cipher) => cipher.read_block(&mut self.stream)?,
            None => read_smp_block(&mut self.stream)?,
        })
    }
}

/// Read and authenticate the server hello, then send a client hello carrying
/// a fresh ephemeral DH key. The server identity fingerprint comes from the
/// parsed SimpleX server address.
pub fn complete_smp_handshake(
    mut stream: SmpTlsStream,
    expected_fingerprint: &[u8; 32],
    supported: SmpVersionRange,
    proxy_server: bool,
) -> Result<SmpHandshake, HandshakeError> {
    let server_block = read_smp_block(&mut stream)?;
    let hello = ServerHello::parse(&server_block)?;
    let version = hello.select_version(stream.tls_unique(), supported, proxy_server)?;
    if version < 7 {
        return Err(HandshakeError::UnsupportedLegacyVersion);
    }
    let auth = hello.unverified_authentication()?;
    let peer_certificates: Vec<&[u8]> = stream
        .peer_certificates()
        .iter()
        .map(Vec::as_slice)
        .collect();
    let server_key = auth.verify_signed_key(&peer_certificates, expected_fingerprint)?;
    let (client_key, peer_key) = generate_key_pair(&server_key)?;
    let client_public_der = client_key
        .public_key_to_der()
        .map_err(HandshakeError::Crypto)?;
    let client_hello = encode_client_hello(
        version,
        expected_fingerprint,
        &client_public_der,
        proxy_server,
    )?;
    let shared_secret = derive_secret(&client_key, &peer_key)?;
    let block_cipher = if version >= 11 && !proxy_server {
        Some(SmpBlockCipher::for_client(
            stream.tls_unique(),
            &shared_secret,
        )?)
    } else {
        None
    };
    write_smp_block(&mut stream, &client_hello)?;
    Ok(SmpHandshake {
        stream,
        version,
        shared_secret,
        block_cipher,
    })
}

fn generate_key_pair(
    server_key: &VerifiedDhKey,
) -> Result<(PKey<Private>, PKey<Public>), HandshakeError> {
    let (client, key_type, bytes) = match server_key {
        VerifiedDhKey::X25519(bytes) => (PKey::generate_x25519(), Id::X25519, bytes),
        VerifiedDhKey::X448(bytes) => (PKey::generate_x448(), Id::X448, bytes),
    };
    let client = client.map_err(HandshakeError::Crypto)?;
    let peer = PKey::public_key_from_raw_bytes(bytes, key_type).map_err(HandshakeError::Crypto)?;
    Ok((client, peer))
}

fn derive_secret(client: &PKey<Private>, peer: &PKey<Public>) -> Result<Vec<u8>, HandshakeError> {
    let mut deriver = Deriver::new(client).map_err(HandshakeError::Crypto)?;
    deriver.set_peer(peer).map_err(HandshakeError::Crypto)?;
    deriver.derive_to_vec().map_err(HandshakeError::Crypto)
}

fn encode_client_hello(
    version: u16,
    fingerprint: &[u8; 32],
    public_key_der: &[u8],
    proxy_server: bool,
) -> Result<Vec<u8>, HandshakeError> {
    let key_len =
        u8::try_from(public_key_der.len()).map_err(|_| HandshakeError::PublicKeyTooLarge)?;
    let mut hello = Vec::with_capacity(2 + 1 + fingerprint.len() + 1 + public_key_der.len() + 2);
    hello.extend_from_slice(&version.to_be_bytes());
    hello.push(fingerprint.len() as u8);
    hello.extend_from_slice(fingerprint);
    if version >= 7 {
        hello.push(key_len);
        hello.extend_from_slice(public_key_der);
    }
    if version >= 14 {
        hello.push(if proxy_server { b'T' } else { b'F' });
    }
    if version >= 16 {
        hello.push(b'0'); // no long-term service certificate
    }
    Ok(hello)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        encrypted::SmpBlockCipher,
        tls::{connect_smp_tls, tests::certificate},
        transport::{read_smp_block, write_smp_block},
    };
    use openssl::{
        pkey::PKey,
        sign::Signer,
        ssl::{SslAcceptor, SslMethod},
    };
    use sha2::{Digest, Sha256};
    use std::{
        net::{TcpListener, TcpStream},
        thread,
    };

    fn tlv(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![tag];
        if payload.len() < 128 {
            bytes.push(payload.len() as u8);
        } else if payload.len() < 256 {
            bytes.extend_from_slice(&[0x81, payload.len() as u8]);
        } else {
            bytes.extend_from_slice(&[0x82, (payload.len() >> 8) as u8, payload.len() as u8]);
        }
        bytes.extend_from_slice(payload);
        bytes
    }

    fn large(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = (bytes.len() as u16).to_be_bytes().to_vec();
        encoded.extend_from_slice(bytes);
        encoded
    }

    #[test]
    fn encodes_versioned_client_hello_fields() {
        let fingerprint = [7; 32];
        let public_key = [0x30, 0x02, 1, 2];
        let v6 = encode_client_hello(6, &fingerprint, &public_key, false).unwrap();
        assert_eq!(&v6[..3], &[0, 6, 32]);
        assert_eq!(v6.len(), 35);

        let v10 = encode_client_hello(10, &fingerprint, &public_key, false).unwrap();
        assert_eq!(&v10[35..], &[4, 0x30, 0x02, 1, 2]);
        let v20 = encode_client_hello(20, &fingerprint, &public_key, true).unwrap();
        assert_eq!(&v20[35..], &[4, 0x30, 0x02, 1, 2, b'T', b'0']);
    }

    #[test]
    fn ephemeral_key_agreement_matches_both_sides() {
        let server = PKey::generate_x25519().unwrap();
        let (client, peer) =
            generate_key_pair(&VerifiedDhKey::X25519(server.raw_public_key().unwrap())).unwrap();
        let client_public =
            PKey::public_key_from_der(&client.public_key_to_der().unwrap()).unwrap();
        let ours = derive_secret(&client, &peer).unwrap();
        let theirs = derive_secret(&server, &client_public).unwrap();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn completes_handshake_with_local_tls_relay() {
        let identity_key = PKey::generate_ed25519().unwrap();
        let identity = certificate(1, "identity", &identity_key, None);
        let leaf_key = PKey::generate_ed25519().unwrap();
        let leaf = certificate(2, "localhost", &leaf_key, Some((&identity, &identity_key)));
        let fingerprint: [u8; 32] = Sha256::digest(identity.to_der().unwrap()).into();
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server()).unwrap();
        acceptor.set_certificate(&leaf).unwrap();
        acceptor.set_private_key(&leaf_key).unwrap();
        acceptor.add_extra_chain_cert(identity.clone()).unwrap();
        acceptor.check_private_key().unwrap();
        let acceptor = acceptor.build();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let mut tls = acceptor.accept(tcp).unwrap();
            let mut session = [0u8; 64];
            let session_len = tls.ssl().peer_finished(&mut session);
            let dh_key = PKey::generate_x25519().unwrap();
            let subject = tlv(0x30, &dh_key.public_key_to_der().unwrap());
            let signature = Signer::new_without_digest(&leaf_key)
                .unwrap()
                .sign_oneshot_to_vec(&subject)
                .unwrap();
            let mut bits = vec![0];
            bits.extend_from_slice(&signature);
            let mut signed_contents = subject;
            signed_contents.extend_from_slice(&tlv(0x30, &[0x06, 0x03, 0x2b, 0x65, 0x70]));
            signed_contents.extend_from_slice(&tlv(0x03, &bits));
            let signed_key = tlv(0x30, &signed_contents);
            let mut hello = SmpVersionRange::CLIENT_SUPPORTED.encode().to_vec();
            hello.push(session_len as u8);
            hello.extend_from_slice(&session[..session_len]);
            hello.push(2);
            hello.extend_from_slice(&large(&leaf.to_der().unwrap()));
            hello.extend_from_slice(&large(&identity.to_der().unwrap()));
            hello.extend_from_slice(&large(&signed_key));
            write_smp_block(&mut tls, &hello).unwrap();
            let response = read_smp_block(&mut tls).unwrap();
            assert_eq!(&response[..3], &[0, 20, 32]);
            assert_eq!(&response[3..35], &fingerprint);
            assert_eq!(&response[response.len() - 2..], b"F0");
            let key_len = response[35] as usize;
            let client_key = PKey::public_key_from_der(&response[36..36 + key_len]).unwrap();
            let shared_secret = derive_secret(&dh_key, &client_key).unwrap();
            let mut cipher =
                SmpBlockCipher::for_server(&session[..session_len], &shared_secret).unwrap();
            cipher.write_block(&mut tls, b"relay-ready").unwrap();
            assert_eq!(cipher.read_block(&mut tls).unwrap(), b"client-ready");
        });

        let tcp = TcpStream::connect(address).unwrap();
        let tls = connect_smp_tls(tcp, "localhost", &fingerprint).unwrap();
        let mut handshake =
            complete_smp_handshake(tls, &fingerprint, SmpVersionRange::CLIENT_SUPPORTED, false)
                .unwrap();
        assert_eq!(handshake.version, 20);
        assert_eq!(handshake.shared_secret().len(), 32);
        assert_eq!(handshake.receive_block().unwrap(), b"relay-ready");
        handshake.send_block(b"client-ready").unwrap();
        server.join().unwrap();
    }
}
