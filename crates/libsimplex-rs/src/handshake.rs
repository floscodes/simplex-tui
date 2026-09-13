//! Authenticated SMP client handshake over a pinned TLS stream.

use std::io;

use openssl::{
    derive::Deriver,
    pkey::{Id, PKey, PKeyRef, Private, Public},
};
use thiserror::Error;

use crate::{
    agent::{ResponseRouter, RoutedTransmission},
    auth::{AuthError, VerifiedDhKey},
    encrypted::{EncryptedBlockError, SmpBlockCipher},
    queue::{self, SubscriptionMode},
    smp::{self, ClientCommand, OwnedTransmission, RawTransmission, SmpWireError},
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
    #[error("SMP transmission encoding failed: {0}")]
    Wire(#[from] SmpWireError),
}

/// Result of the initial handshake. Further SMP traffic requires the agent
/// protocol and, for v11+, encrypted transport blocks.
pub struct SmpHandshake {
    stream: SmpTlsStream,
    pub version: u16,
    shared_secret: Vec<u8>,
    server_dh_key: PKey<Public>,
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

    /// Send the unauthenticated SMP keepalive using the negotiated session
    /// framing. The caller supplies a correlation ID to match its reply.
    pub fn send_ping(&mut self, correlation_id: &[u8]) -> Result<(), HandshakeError> {
        let raw = RawTransmission {
            authenticator: b"",
            service_signature: None,
            session_id: self.stream.tls_unique(),
            correlation_id,
            entity_id: b"",
            command: b"PING",
        };
        let transmission = smp::encode_transmission(raw, self.version >= 7, self.version >= 16)?;
        let batch = smp::encode_batch(&[transmission])?;
        self.send_block(&batch)
    }

    /// Send a signed base `NEW` request. The caller owns the generated keys
    /// and must persist them together with the eventual `IDS` reply.
    pub fn send_new_queue(
        &mut self,
        correlation_id: &[u8],
        recipient_auth_key: &PKeyRef<Private>,
        recipient_dh_key: &PKeyRef<Public>,
        subscription: SubscriptionMode,
    ) -> Result<(), HandshakeError> {
        let transmission = queue::sign_new_queue(
            self.version,
            self.stream.tls_unique(),
            correlation_id,
            recipient_auth_key,
            recipient_dh_key,
            subscription,
        )?;
        let batch = smp::encode_batch(&[transmission])?;
        self.send_block(&batch)
    }

    /// Send an entity-authorized queue command with an Ed25519/Ed448 key.
    /// The caller must use the key registered for `entity_id` and a unique
    /// correlation ID for each in-flight request.
    pub fn send_signed_queue_command(
        &mut self,
        correlation_id: &[u8],
        entity_id: &[u8],
        command: ClientCommand<'_>,
        key: &PKeyRef<Private>,
    ) -> Result<(), HandshakeError> {
        if entity_id.is_empty() || matches!(command, ClientCommand::Ping) {
            return Err(SmpWireError::InvalidAuthentication.into());
        }
        let transmission = smp::sign_transmission(
            self.stream.tls_unique(),
            correlation_id,
            entity_id,
            command,
            key,
            self.version >= 7,
            self.version >= 16,
        )?;
        self.send_block(&smp::encode_batch(&[transmission])?)
    }

    /// Send a queue command authenticated with the registered X25519 key and
    /// the server key validated during the handshake.
    pub fn send_x25519_queue_command(
        &mut self,
        correlation_id: &[u8],
        entity_id: &[u8],
        command: ClientCommand<'_>,
        key: &PKeyRef<Private>,
    ) -> Result<(), HandshakeError> {
        if entity_id.is_empty() || matches!(command, ClientCommand::Ping) {
            return Err(SmpWireError::InvalidAuthentication.into());
        }
        let transmission = smp::X25519TransmissionAuthenticator {
            private_key: key,
            peer_key: &self.server_dh_key,
            imply_session_id: self.version >= 7,
            service_auth: self.version >= 16,
        }
        .authenticate(self.stream.tls_unique(), correlation_id, entity_id, command)?;
        self.send_block(&smp::encode_batch(&[transmission])?)
    }

    /// Decode all transmissions in the next SMP transport block. Command
    /// bodies remain raw because the agent must handle asynchronous responses.
    pub fn receive_transmissions(&mut self) -> Result<Vec<OwnedTransmission>, HandshakeError> {
        let block = self.receive_block()?;
        let batch = smp::parse_batch(&block)?;
        batch
            .into_iter()
            .map(|bytes| {
                smp::parse_transmission(
                    bytes,
                    self.stream.tls_unique(),
                    self.version >= 7,
                    self.version >= 16,
                )
                .map(OwnedTransmission::from)
                .map_err(HandshakeError::from)
            })
            .collect()
    }

    /// Read one batch and classify each item against outstanding requests.
    pub fn receive_routed(
        &mut self,
        router: &mut ResponseRouter,
    ) -> Result<Vec<RoutedTransmission>, HandshakeError> {
        Ok(router.route_batch(self.receive_transmissions()?))
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
        server_dh_key: peer_key,
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
        let queue_auth = PKey::generate_ed25519().unwrap();
        let queue_auth_public =
            PKey::public_key_from_der(&queue_auth.public_key_to_der().unwrap()).unwrap();
        let queue_dh = PKey::generate_x25519().unwrap();
        let queue_dh_public =
            PKey::public_key_from_der(&queue_dh.public_key_to_der().unwrap()).unwrap();
        let queue_x25519_auth = PKey::generate_x25519().unwrap();
        let queue_x25519_public =
            PKey::public_key_from_der(&queue_x25519_auth.public_key_to_der().unwrap()).unwrap();
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
            let ping_batch = cipher.read_block(&mut tls).unwrap();
            let ping = crate::smp::parse_batch(&ping_batch).unwrap();
            assert_eq!(ping.len(), 1);
            let ping = crate::smp::parse_transmission(ping[0], &session[..session_len], true, true)
                .unwrap();
            assert_eq!(ping.correlation_id, b"ping-1");
            assert_eq!(ping.command, b"PING");
            let pong = crate::smp::encode_transmission(
                crate::smp::RawTransmission {
                    authenticator: b"",
                    service_signature: None,
                    session_id: &session[..session_len],
                    correlation_id: b"ping-1",
                    entity_id: b"",
                    command: b"PONG",
                },
                true,
                true,
            )
            .unwrap();
            cipher
                .write_block(&mut tls, &crate::smp::encode_batch(&[pong]).unwrap())
                .unwrap();
            let new_batch = cipher.read_block(&mut tls).unwrap();
            let new_batch = crate::smp::parse_batch(&new_batch).unwrap();
            assert_eq!(new_batch.len(), 1);
            let new_queue =
                crate::smp::parse_transmission(new_batch[0], &session[..session_len], true, true)
                    .unwrap();
            assert_eq!(new_queue.entity_id, b"");
            assert_eq!(new_queue.correlation_id, b"new-1");
            assert!(new_queue.command.starts_with(b"NEW "));
            assert_eq!(
                crate::smp::verify_transmission_signature(new_queue, &queue_auth_public),
                Ok(true)
            );
            let mut ids = b"IDS ".to_vec();
            crate::smp::put_short(&mut ids, b"recipient-id").unwrap();
            crate::smp::put_short(&mut ids, b"sender-id").unwrap();
            crate::smp::put_short(&mut ids, &dh_key.public_key_to_der().unwrap()).unwrap();
            ids.extend_from_slice(b"0000");
            let ids = crate::smp::encode_transmission(
                crate::smp::RawTransmission {
                    authenticator: b"",
                    service_signature: None,
                    session_id: &session[..session_len],
                    correlation_id: b"new-1",
                    entity_id: b"",
                    command: &ids,
                },
                true,
                true,
            )
            .unwrap();
            cipher
                .write_block(&mut tls, &crate::smp::encode_batch(&[ids]).unwrap())
                .unwrap();
            for (expected_corr, expected_command) in [
                (b"sub-1".as_slice(), b"SUB".as_slice()),
                (b"get-1".as_slice(), b"GET".as_slice()),
            ] {
                let batch = cipher.read_block(&mut tls).unwrap();
                let batch = crate::smp::parse_batch(&batch).unwrap();
                assert_eq!(batch.len(), 1);
                let request =
                    crate::smp::parse_transmission(batch[0], &session[..session_len], true, true)
                        .unwrap();
                assert_eq!(request.correlation_id, expected_corr);
                assert_eq!(request.entity_id, b"recipient-id");
                assert_eq!(request.command, expected_command);
                let verified = if expected_command == b"SUB" {
                    crate::smp::verify_transmission_signature(request, &queue_auth_public)
                } else {
                    crate::smp::verify_x25519_authenticator(request, &dh_key, &queue_x25519_public)
                };
                assert_eq!(verified, Ok(true));
                let ok = crate::smp::encode_transmission(
                    crate::smp::RawTransmission {
                        authenticator: b"",
                        service_signature: None,
                        session_id: &session[..session_len],
                        correlation_id: expected_corr,
                        entity_id: b"recipient-id",
                        command: b"OK",
                    },
                    true,
                    true,
                )
                .unwrap();
                cipher
                    .write_block(&mut tls, &crate::smp::encode_batch(&[ok]).unwrap())
                    .unwrap();
                if expected_command == b"SUB" {
                    let event = crate::smp::encode_transmission(
                        crate::smp::RawTransmission {
                            authenticator: b"",
                            service_signature: None,
                            session_id: &session[..session_len],
                            correlation_id: b"",
                            entity_id: b"recipient-id",
                            command: b"MSG \x03msgciphertext",
                        },
                        true,
                        true,
                    )
                    .unwrap();
                    cipher
                        .write_block(&mut tls, &crate::smp::encode_batch(&[event]).unwrap())
                        .unwrap();
                }
            }
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
        handshake.send_ping(b"ping-1").unwrap();
        let replies = handshake.receive_transmissions().unwrap();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].correlation_id, b"ping-1");
        assert_eq!(
            crate::smp::BrokerResponse::parse(&replies[0].command),
            Ok(crate::smp::BrokerResponse::Pong)
        );
        handshake
            .send_new_queue(
                b"new-1",
                &queue_auth,
                &queue_dh_public,
                SubscriptionMode::CreateOnly,
            )
            .unwrap();
        let ids_reply = handshake.receive_transmissions().unwrap();
        assert_eq!(ids_reply.len(), 1);
        let ids = crate::queue::parse_queue_ids(handshake.version, &ids_reply[0].command).unwrap();
        assert_eq!(ids.recipient_id, b"recipient-id");
        assert_eq!(ids.sender_id, b"sender-id");
        let mut router = ResponseRouter::new();
        router.register(b"sub-1", b"recipient-id", b"SUB").unwrap();
        handshake
            .send_signed_queue_command(
                b"sub-1",
                b"recipient-id",
                ClientCommand::Subscribe,
                &queue_auth,
            )
            .unwrap();
        let reply = handshake.receive_routed(&mut router).unwrap();
        assert!(
            matches!(&reply[0], RoutedTransmission::Response { request, transmission }
            if request.command == b"SUB" && transmission.correlation_id == b"sub-1"
                && crate::smp::BrokerResponse::parse(&transmission.command) == Ok(crate::smp::BrokerResponse::Ok))
        );
        let event = handshake.receive_routed(&mut router).unwrap();
        assert!(matches!(&event[0], RoutedTransmission::Event(transmission)
            if transmission.entity_id == b"recipient-id"
                && crate::smp::BrokerResponse::parse(&transmission.command)
                    == Ok(crate::smp::BrokerResponse::Message { message_id: b"msg", body: b"ciphertext" })));
        router.register(b"get-1", b"recipient-id", b"GET").unwrap();
        handshake
            .send_x25519_queue_command(
                b"get-1",
                b"recipient-id",
                ClientCommand::Get,
                &queue_x25519_auth,
            )
            .unwrap();
        let reply = handshake.receive_routed(&mut router).unwrap();
        assert!(
            matches!(&reply[0], RoutedTransmission::Response { request, transmission }
            if request.command == b"GET" && transmission.correlation_id == b"get-1"
                && crate::smp::BrokerResponse::parse(&transmission.command) == Ok(crate::smp::BrokerResponse::Ok))
        );
        server.join().unwrap();
    }
}
