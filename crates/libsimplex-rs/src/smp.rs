//! Raw SMP transmissions, transport batches, and a typed message-flow subset.
//!
//! This layer deliberately leaves signatures and command bodies opaque. The
//! caller must authenticate commands before using them as agent state changes.

use crypto_secretbox::{
    Key, Nonce, XSalsa20Poly1305,
    aead::{Aead, KeyInit},
};
use openssl::{
    derive::Deriver,
    pkey::{Id, PKeyRef, Private, Public},
    sign::{Signer, Verifier},
};
use sha2::{Digest, Sha512};
use thiserror::Error;

use crate::transport::SMP_BLOCK_SIZE;

const BATCH_OVERHEAD: usize = 2 + 1 + 16; // padding length, count, secretbox tag

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawTransmission<'a> {
    pub authenticator: &'a [u8],
    pub service_signature: Option<&'a [u8]>,
    pub session_id: &'a [u8],
    pub correlation_id: &'a [u8],
    pub entity_id: &'a [u8],
    pub command: &'a [u8],
}

/// Owned transmission returned after a transport block has been consumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedTransmission {
    pub authenticator: Vec<u8>,
    pub service_signature: Option<Vec<u8>>,
    pub session_id: Vec<u8>,
    pub correlation_id: Vec<u8>,
    pub entity_id: Vec<u8>,
    pub command: Vec<u8>,
}

impl From<RawTransmission<'_>> for OwnedTransmission {
    fn from(raw: RawTransmission<'_>) -> Self {
        Self {
            authenticator: raw.authenticator.to_vec(),
            service_signature: raw.service_signature.map(<[u8]>::to_vec),
            session_id: raw.session_id.to_vec(),
            correlation_id: raw.correlation_id.to_vec(),
            entity_id: raw.entity_id.to_vec(),
            command: raw.command.to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SmpWireError {
    #[error("SMP field exceeds 255 bytes")]
    FieldTooLong,
    #[error("SMP transmission or batch exceeds block capacity")]
    TooLarge,
    #[error("SMP transmission or batch is malformed")]
    Malformed,
    #[error("SMP transmission has the wrong TLS session ID")]
    SessionMismatch,
    #[error("unsupported SMP command or response")]
    UnknownTag,
    #[error("SMP authentication fields are invalid for this version")]
    InvalidAuthentication,
    #[error("SMP signature operation failed")]
    SignatureFailure,
}

/// Client commands needed for a basic queue subscription and message flow.
/// Key setup and authenticated queue creation are separate agent operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientCommand<'a> {
    Ping,
    Subscribe,
    Get,
    Ack { message_id: &'a [u8] },
    Send { notification: bool, body: &'a [u8] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrokerResponse<'a> {
    Pong,
    Ok,
    End,
    Deleted,
    Message {
        message_id: &'a [u8],
        body: &'a [u8],
    },
    Error(&'a [u8]),
}

impl ClientCommand<'_> {
    pub fn encode(self) -> Result<Vec<u8>, SmpWireError> {
        let mut out = Vec::new();
        match self {
            Self::Ping => out.extend_from_slice(b"PING"),
            Self::Subscribe => out.extend_from_slice(b"SUB"),
            Self::Get => out.extend_from_slice(b"GET"),
            Self::Ack { message_id } => {
                out.extend_from_slice(b"ACK ");
                put_short(&mut out, message_id)?;
            }
            Self::Send { notification, body } => {
                out.extend_from_slice(b"SEND ");
                out.push(if notification { b'T' } else { b'F' });
                out.push(b' ');
                out.extend_from_slice(body);
            }
        }
        Ok(out)
    }
}

impl<'a> ClientCommand<'a> {
    pub fn parse(input: &'a [u8]) -> Result<Self, SmpWireError> {
        match input {
            b"PING" => Ok(Self::Ping),
            b"SUB" => Ok(Self::Subscribe),
            b"GET" => Ok(Self::Get),
            _ => {
                let (tag, parameters) = input
                    .split_once_byte(b' ')
                    .ok_or(SmpWireError::UnknownTag)?;
                match tag {
                    b"ACK" => {
                        let mut remaining = parameters;
                        let message_id = take_short(&mut remaining)?;
                        if !remaining.is_empty() {
                            return Err(SmpWireError::Malformed);
                        }
                        Ok(Self::Ack { message_id })
                    }
                    b"SEND" => {
                        let (&flag, body) =
                            parameters.split_first().ok_or(SmpWireError::Malformed)?;
                        let (&separator, body) =
                            body.split_first().ok_or(SmpWireError::Malformed)?;
                        if separator != b' ' {
                            return Err(SmpWireError::Malformed);
                        }
                        let notification = match flag {
                            b'T' => true,
                            b'F' => false,
                            _ => return Err(SmpWireError::Malformed),
                        };
                        Ok(Self::Send { notification, body })
                    }
                    _ => Err(SmpWireError::UnknownTag),
                }
            }
        }
    }
}

impl<'a> BrokerResponse<'a> {
    pub fn parse(input: &'a [u8]) -> Result<Self, SmpWireError> {
        match input {
            b"PONG" => Ok(Self::Pong),
            b"OK" => Ok(Self::Ok),
            b"END" => Ok(Self::End),
            b"DELD" => Ok(Self::Deleted),
            _ => {
                let (tag, parameters) = input
                    .split_once_byte(b' ')
                    .ok_or(SmpWireError::UnknownTag)?;
                match tag {
                    b"MSG" => {
                        let mut parameters = parameters;
                        let message_id = take_short(&mut parameters)?;
                        Ok(Self::Message {
                            message_id,
                            body: parameters,
                        })
                    }
                    b"ERR" if !parameters.is_empty() => Ok(Self::Error(parameters)),
                    b"ERR" => Err(SmpWireError::Malformed),
                    _ => Err(SmpWireError::UnknownTag),
                }
            }
        }
    }
}

trait SplitOnceByte {
    fn split_once_byte(&self, delimiter: u8) -> Option<(&[u8], &[u8])>;
}

impl SplitOnceByte for [u8] {
    fn split_once_byte(&self, delimiter: u8) -> Option<(&[u8], &[u8])> {
        let position = self.iter().position(|byte| *byte == delimiter)?;
        Some((&self[..position], &self[position + 1..]))
    }
}

pub(crate) fn put_short(out: &mut Vec<u8>, value: &[u8]) -> Result<(), SmpWireError> {
    out.push(u8::try_from(value.len()).map_err(|_| SmpWireError::FieldTooLong)?);
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn take_short<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], SmpWireError> {
    let (&length, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
    let (value, tail) = rest
        .split_at_checked(length as usize)
        .ok_or(SmpWireError::Malformed)?;
    *input = tail;
    Ok(value)
}

/// Bytes signed by an SMP entity key: the length-prefixed TLS session ID,
/// correlation ID, entity ID, and encoded command. The session ID is included
/// in the signature even when v7+ omits it from the transmitted fields.
pub fn authorization_bytes(
    session_id: &[u8],
    correlation_id: &[u8],
    entity_id: &[u8],
    command: &[u8],
) -> Result<Vec<u8>, SmpWireError> {
    let mut bytes = Vec::new();
    put_short(&mut bytes, session_id)?;
    put_short(&mut bytes, correlation_id)?;
    put_short(&mut bytes, entity_id)?;
    bytes.extend_from_slice(command);
    Ok(bytes)
}

/// Sign a client transmission with an Ed25519 or Ed448 entity key. This does
/// not implement the X25519 authenticator or the optional service signature.
pub fn sign_transmission(
    session_id: &[u8],
    correlation_id: &[u8],
    entity_id: &[u8],
    command: ClientCommand<'_>,
    key: &PKeyRef<Private>,
    imply_session_id: bool,
    service_auth: bool,
) -> Result<Vec<u8>, SmpWireError> {
    sign_encoded_transmission(
        session_id,
        correlation_id,
        entity_id,
        &command.encode()?,
        key,
        imply_session_id,
        service_auth,
    )
}

/// Sign an already encoded SMP command, including queue-creation commands.
pub fn sign_encoded_transmission(
    session_id: &[u8],
    correlation_id: &[u8],
    entity_id: &[u8],
    command: &[u8],
    key: &PKeyRef<Private>,
    imply_session_id: bool,
    service_auth: bool,
) -> Result<Vec<u8>, SmpWireError> {
    if !matches!(key.id(), Id::ED25519 | Id::ED448) {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let authorized = authorization_bytes(session_id, correlation_id, entity_id, command)?;
    let mut signer = Signer::new_without_digest(key).map_err(|_| SmpWireError::SignatureFailure)?;
    let signature = signer
        .sign_oneshot_to_vec(&authorized)
        .map_err(|_| SmpWireError::SignatureFailure)?;
    encode_transmission(
        RawTransmission {
            authenticator: &signature,
            service_signature: None,
            session_id,
            correlation_id,
            entity_id,
            command,
        },
        imply_session_id,
        service_auth,
    )
}

/// Verify an entity-key signature on a parsed transmission. A caller must
/// obtain the public key from an authenticated queue record.
pub fn verify_transmission_signature(
    transmission: RawTransmission<'_>,
    key: &PKeyRef<Public>,
) -> Result<bool, SmpWireError> {
    let expected_length = match key.id() {
        Id::ED25519 => 64,
        Id::ED448 => 114,
        _ => return Err(SmpWireError::InvalidAuthentication),
    };
    if transmission.authenticator.len() != expected_length {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let authorized = authorization_bytes(
        transmission.session_id,
        transmission.correlation_id,
        transmission.entity_id,
        transmission.command,
    )?;
    let mut verifier =
        Verifier::new_without_digest(key).map_err(|_| SmpWireError::SignatureFailure)?;
    verifier
        .verify_oneshot(transmission.authenticator, &authorized)
        .map_err(|_| SmpWireError::SignatureFailure)
}

/// Keys and pinned service-certificate hash for a v16+ service subscription.
/// `service_key` must be Ed25519, as in the pinned upstream protocol.
pub struct ServiceSubscriptionSigner<'a> {
    pub entity_key: &'a PKeyRef<Private>,
    pub service_key: &'a PKeyRef<Private>,
    pub service_cert_hash: &'a [u8; 32],
}

impl ServiceSubscriptionSigner<'_> {
    /// Sign `SUB` exactly as `authTransmission` does: the service key signs
    /// the authorized transmission and the entity key signs the fingerprint
    /// followed by that same transmission.
    pub fn sign(
        &self,
        session_id: &[u8],
        correlation_id: &[u8],
        entity_id: &[u8],
    ) -> Result<Vec<u8>, SmpWireError> {
        if !matches!(self.entity_key.id(), Id::ED25519 | Id::ED448)
            || self.service_key.id() != Id::ED25519
        {
            return Err(SmpWireError::InvalidAuthentication);
        }
        let authorized = authorization_bytes(session_id, correlation_id, entity_id, b"SUB")?;
        let mut service_signer = Signer::new_without_digest(self.service_key)
            .map_err(|_| SmpWireError::SignatureFailure)?;
        let service_signature = service_signer
            .sign_oneshot_to_vec(&authorized)
            .map_err(|_| SmpWireError::SignatureFailure)?;
        let mut entity_input = Vec::with_capacity(32 + authorized.len());
        entity_input.extend_from_slice(self.service_cert_hash);
        entity_input.extend_from_slice(&authorized);
        let mut entity_signer = Signer::new_without_digest(self.entity_key)
            .map_err(|_| SmpWireError::SignatureFailure)?;
        let entity_signature = entity_signer
            .sign_oneshot_to_vec(&entity_input)
            .map_err(|_| SmpWireError::SignatureFailure)?;
        encode_transmission(
            RawTransmission {
                authenticator: &entity_signature,
                service_signature: Some(&service_signature),
                session_id,
                correlation_id,
                entity_id,
                command: b"SUB",
            },
            true,
            true,
        )
    }
}

/// Verify both signatures for a v16+ service subscription. The caller must
/// bind `service_cert_hash` and both public keys to the authenticated service
/// and queue records before trusting the result.
pub fn verify_service_subscription(
    transmission: RawTransmission<'_>,
    entity_key: &PKeyRef<Public>,
    service_key: &PKeyRef<Public>,
    service_cert_hash: &[u8; 32],
) -> Result<bool, SmpWireError> {
    if transmission.command != b"SUB"
        || !matches!(entity_key.id(), Id::ED25519 | Id::ED448)
        || service_key.id() != Id::ED25519
    {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let service_signature = transmission
        .service_signature
        .ok_or(SmpWireError::InvalidAuthentication)?;
    let entity_length = if entity_key.id() == Id::ED25519 {
        64
    } else {
        114
    };
    if transmission.authenticator.len() != entity_length || service_signature.len() != 64 {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let authorized = authorization_bytes(
        transmission.session_id,
        transmission.correlation_id,
        transmission.entity_id,
        transmission.command,
    )?;
    let mut service_verifier =
        Verifier::new_without_digest(service_key).map_err(|_| SmpWireError::SignatureFailure)?;
    if !service_verifier
        .verify_oneshot(service_signature, &authorized)
        .map_err(|_| SmpWireError::SignatureFailure)?
    {
        return Ok(false);
    }
    let mut entity_input = Vec::with_capacity(32 + authorized.len());
    entity_input.extend_from_slice(service_cert_hash);
    entity_input.extend_from_slice(&authorized);
    let mut entity_verifier =
        Verifier::new_without_digest(entity_key).map_err(|_| SmpWireError::SignatureFailure)?;
    entity_verifier
        .verify_oneshot(transmission.authenticator, &entity_input)
        .map_err(|_| SmpWireError::SignatureFailure)
}

fn x25519_shared_secret(
    private_key: &PKeyRef<Private>,
    peer_key: &PKeyRef<Public>,
) -> Result<[u8; 32], SmpWireError> {
    if private_key.id() != Id::X25519 || peer_key.id() != Id::X25519 {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let mut deriver = Deriver::new(private_key).map_err(|_| SmpWireError::SignatureFailure)?;
    deriver
        .set_peer(peer_key)
        .map_err(|_| SmpWireError::SignatureFailure)?;
    let secret = deriver
        .derive_to_vec()
        .map_err(|_| SmpWireError::SignatureFailure)?;
    secret
        .try_into()
        .map_err(|_| SmpWireError::InvalidAuthentication)
}

fn correlation_nonce(correlation_id: &[u8]) -> [u8; 24] {
    // `cbNonce`: truncate longer IDs, zero-pad shorter ones.
    let mut nonce = [0u8; 24];
    let length = correlation_id.len().min(24);
    nonce[..length].copy_from_slice(&correlation_id[..length]);
    nonce
}

/// X25519 `CbAuthenticator`: XSalsa20-Poly1305 encryption of SHA-512 over
/// the authorized bytes, using the correlation ID as the 24-byte nonce.
pub fn x25519_authenticator(
    authorized: &[u8],
    correlation_id: &[u8],
    private_key: &PKeyRef<Private>,
    peer_key: &PKeyRef<Public>,
) -> Result<Vec<u8>, SmpWireError> {
    let secret = x25519_shared_secret(private_key, peer_key)?;
    let nonce = correlation_nonce(correlation_id);
    XSalsa20Poly1305::new(Key::from_slice(&secret))
        .encrypt(
            Nonce::from_slice(&nonce),
            Sha512::digest(authorized).as_slice(),
        )
        .map_err(|_| SmpWireError::SignatureFailure)
}

/// Verify an X25519 authenticator with the peer's queue key. Both public keys
/// must come from an authenticated queue or server record.
pub fn verify_x25519_authenticator(
    transmission: RawTransmission<'_>,
    private_key: &PKeyRef<Private>,
    peer_key: &PKeyRef<Public>,
) -> Result<bool, SmpWireError> {
    if transmission.authenticator.len() != 80 || transmission.service_signature.is_some() {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let secret = x25519_shared_secret(private_key, peer_key)?;
    let nonce = correlation_nonce(transmission.correlation_id);
    let plaintext = match XSalsa20Poly1305::new(Key::from_slice(&secret))
        .decrypt(Nonce::from_slice(&nonce), transmission.authenticator)
    {
        Ok(plaintext) => plaintext,
        Err(_) => return Ok(false),
    };
    let authorized = authorization_bytes(
        transmission.session_id,
        transmission.correlation_id,
        transmission.entity_id,
        transmission.command,
    )?;
    Ok(openssl::memcmp::eq(
        &plaintext,
        Sha512::digest(&authorized).as_slice(),
    ))
}

/// An X25519 queue key and its peer key for client command authentication.
pub struct X25519TransmissionAuthenticator<'a> {
    pub private_key: &'a PKeyRef<Private>,
    pub peer_key: &'a PKeyRef<Public>,
    pub imply_session_id: bool,
    pub service_auth: bool,
}

impl X25519TransmissionAuthenticator<'_> {
    /// Correlation IDs must be unique for a key pair: upstream uses them as
    /// XSalsa20-Poly1305 nonces.
    pub fn authenticate(
        &self,
        session_id: &[u8],
        correlation_id: &[u8],
        entity_id: &[u8],
        command: ClientCommand<'_>,
    ) -> Result<Vec<u8>, SmpWireError> {
        let command = command.encode()?;
        let authorized = authorization_bytes(session_id, correlation_id, entity_id, &command)?;
        let authenticator =
            x25519_authenticator(&authorized, correlation_id, self.private_key, self.peer_key)?;
        encode_transmission(
            RawTransmission {
                authenticator: &authenticator,
                service_signature: None,
                session_id,
                correlation_id,
                entity_id,
                command: &command,
            },
            self.imply_session_id,
            self.service_auth,
        )
    }
}

/// Encode one transmission. `imply_session_id` is set by the negotiated
/// transport parameters, never inferred from the contents of a message.
pub fn encode_transmission(
    transmission: RawTransmission<'_>,
    imply_session_id: bool,
    service_auth: bool,
) -> Result<Vec<u8>, SmpWireError> {
    let mut out = Vec::new();
    if transmission.authenticator.is_empty() && transmission.service_signature.is_some() {
        return Err(SmpWireError::InvalidAuthentication);
    }
    put_short(&mut out, transmission.authenticator)?;
    if service_auth && !transmission.authenticator.is_empty() {
        match transmission.service_signature {
            Some(signature) if signature.len() == 64 => {
                out.push(b'1');
                put_short(&mut out, signature)?;
            }
            Some(_) => return Err(SmpWireError::InvalidAuthentication),
            None => out.push(b'0'),
        }
    } else if transmission.service_signature.is_some() {
        return Err(SmpWireError::InvalidAuthentication);
    }
    if !imply_session_id {
        put_short(&mut out, transmission.session_id)?;
    }
    put_short(&mut out, transmission.correlation_id)?;
    put_short(&mut out, transmission.entity_id)?;
    out.extend_from_slice(transmission.command);
    Ok(out)
}

/// Parse one transmission from a plaintext or decrypted transport payload.
/// The session check mirrors the upstream client-side transmission decoder.
pub fn parse_transmission<'a>(
    mut input: &'a [u8],
    expected_session_id: &'a [u8],
    imply_session_id: bool,
    service_auth: bool,
) -> Result<RawTransmission<'a>, SmpWireError> {
    let authenticator = take_short(&mut input)?;
    let service_signature = if service_auth && !authenticator.is_empty() {
        let (&tag, tail) = input.split_first().ok_or(SmpWireError::Malformed)?;
        input = tail;
        match tag {
            b'0' => None,
            b'1' => {
                let signature = take_short(&mut input)?;
                if signature.len() != 64 {
                    return Err(SmpWireError::InvalidAuthentication);
                }
                Some(signature)
            }
            _ => return Err(SmpWireError::InvalidAuthentication),
        }
    } else {
        None
    };
    let session_id = if imply_session_id {
        expected_session_id
    } else {
        take_short(&mut input)?
    };
    let correlation_id = take_short(&mut input)?;
    let entity_id = take_short(&mut input)?;
    if session_id != expected_session_id {
        return Err(SmpWireError::SessionMismatch);
    }
    if input.is_empty() {
        return Err(SmpWireError::Malformed);
    }
    Ok(RawTransmission {
        authenticator,
        service_signature,
        session_id,
        correlation_id,
        entity_id,
        command: input,
    })
}

/// Encode an SMP transport batch. Each entry is `Large` (network-order Word16 length)
/// and the leading byte gives the number of transmissions.
pub fn encode_batch(transmissions: &[Vec<u8>]) -> Result<Vec<u8>, SmpWireError> {
    let count = u8::try_from(transmissions.len()).map_err(|_| SmpWireError::TooLarge)?;
    if count == 0 {
        return Err(SmpWireError::Malformed);
    }
    let mut out = vec![count];
    for transmission in transmissions {
        let length = u16::try_from(transmission.len()).map_err(|_| SmpWireError::TooLarge)?;
        if transmission.is_empty() {
            return Err(SmpWireError::Malformed);
        }
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(transmission);
        if out.len() > SMP_BLOCK_SIZE - BATCH_OVERHEAD + 1 {
            return Err(SmpWireError::TooLarge);
        }
    }
    Ok(out)
}

pub fn parse_batch(mut input: &[u8]) -> Result<Vec<&[u8]>, SmpWireError> {
    let (&count, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
    if count == 0 {
        return Err(SmpWireError::Malformed);
    }
    input = rest;
    let mut transmissions = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (&high, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
        let (&low, rest) = rest.split_first().ok_or(SmpWireError::Malformed)?;
        let length = u16::from_be_bytes([high, low]) as usize;
        if length == 0 {
            return Err(SmpWireError::Malformed);
        }
        let (transmission, tail) = rest
            .split_at_checked(length)
            .ok_or(SmpWireError::Malformed)?;
        transmissions.push(transmission);
        input = tail;
    }
    if !input.is_empty() {
        return Err(SmpWireError::Malformed);
    }
    Ok(transmissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::pkey::PKey;

    #[test]
    fn ping_wire_vector_and_batch() {
        let ping = RawTransmission {
            authenticator: b"",
            service_signature: None,
            session_id: b"tls",
            correlation_id: b"id",
            entity_id: b"",
            command: b"PING",
        };
        let encoded = encode_transmission(ping, true, true).unwrap();
        assert_eq!(encoded, b"\0\x02id\0PING");
        assert_eq!(
            parse_transmission(&encoded, b"tls", true, true).unwrap(),
            ping
        );
        let legacy = encode_transmission(ping, false, false).unwrap();
        assert_eq!(legacy, b"\0\x03tls\x02id\0PING");
        assert_eq!(
            parse_transmission(&legacy, b"tls", false, false).unwrap(),
            ping
        );
        let batch = encode_batch(&[encoded.clone(), encoded.clone()]).unwrap();
        assert_eq!(&batch[..3], &[2, 0, encoded.len() as u8]);
        assert_eq!(
            parse_batch(&batch).unwrap(),
            vec![encoded.as_slice(), encoded.as_slice()]
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_messages() {
        assert_eq!(
            parse_transmission(b"\0\x03bad\0\0PING", b"tls", false, false),
            Err(SmpWireError::SessionMismatch)
        );
        assert_eq!(
            parse_transmission(b"\0\x05id", b"tls", true, false),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(parse_batch(b"\x01\0\x04abc"), Err(SmpWireError::Malformed));
        assert_eq!(parse_batch(b"\x01\0\x01xy"), Err(SmpWireError::Malformed));
        assert_eq!(
            encode_batch(&[vec![1; SMP_BLOCK_SIZE]]),
            Err(SmpWireError::TooLarge)
        );
        assert_eq!(encode_batch(&[]), Err(SmpWireError::Malformed));
    }

    #[test]
    fn typed_message_flow_matches_upstream_fields() {
        for (command, wire) in [
            (ClientCommand::Ping, b"PING".as_slice()),
            (ClientCommand::Subscribe, b"SUB"),
            (ClientCommand::Get, b"GET"),
            (ClientCommand::Ack { message_id: b"123" }, b"ACK \x03123"),
            (
                ClientCommand::Send {
                    notification: true,
                    body: b"ciphertext",
                },
                b"SEND T ciphertext",
            ),
            (
                ClientCommand::Send {
                    notification: false,
                    body: b"",
                },
                b"SEND F ",
            ),
        ] {
            assert_eq!(command.encode().unwrap(), wire);
            assert_eq!(ClientCommand::parse(wire).unwrap(), command);
        }
        assert_eq!(BrokerResponse::parse(b"PONG"), Ok(BrokerResponse::Pong));
        assert_eq!(BrokerResponse::parse(b"OK"), Ok(BrokerResponse::Ok));
        assert_eq!(
            BrokerResponse::parse(b"MSG \x03123ciphertext"),
            Ok(BrokerResponse::Message {
                message_id: b"123",
                body: b"ciphertext"
            })
        );
        assert_eq!(
            BrokerResponse::parse(b"ERR AUTH"),
            Ok(BrokerResponse::Error(b"AUTH"))
        );
    }

    #[test]
    fn typed_messages_reject_bad_fields() {
        assert_eq!(
            ClientCommand::parse(b"ACK \x0312"),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(
            ClientCommand::parse(b"ACK \x011extra"),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(
            ClientCommand::parse(b"SEND X data"),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(
            ClientCommand::parse(b"SEND Tdata"),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(
            BrokerResponse::parse(b"MSG \x0412"),
            Err(SmpWireError::Malformed)
        );
        assert_eq!(BrokerResponse::parse(b"ERR "), Err(SmpWireError::Malformed));
        assert_eq!(
            BrokerResponse::parse(b"UNKNOWN"),
            Err(SmpWireError::UnknownTag)
        );
        assert_eq!(
            ClientCommand::Ack {
                message_id: &[0; 256]
            }
            .encode(),
            Err(SmpWireError::FieldTooLong)
        );
    }

    #[test]
    fn signs_session_bound_commands_and_rejects_tampering() {
        for private in [
            PKey::generate_ed25519().unwrap(),
            PKey::generate_ed448().unwrap(),
        ] {
            let public = PKey::public_key_from_der(&private.public_key_to_der().unwrap()).unwrap();
            let wire = sign_transmission(
                b"tls-session",
                b"corr",
                b"queue",
                ClientCommand::Ack { message_id: b"msg" },
                &private,
                true,
                true,
            )
            .unwrap();
            let parsed = parse_transmission(&wire, b"tls-session", true, true).unwrap();
            assert_eq!(
                ClientCommand::parse(parsed.command),
                Ok(ClientCommand::Ack { message_id: b"msg" })
            );
            assert_eq!(verify_transmission_signature(parsed, &public), Ok(true));
            let mut altered = parsed;
            altered.command = b"GET";
            assert_eq!(verify_transmission_signature(altered, &public), Ok(false));
            altered = parsed;
            altered.session_id = b"other-session";
            assert_eq!(verify_transmission_signature(altered, &public), Ok(false));
        }
    }

    #[test]
    fn service_signature_field_is_versioned_and_strict() {
        let signature = [8u8; 64];
        let auth = [9u8; 64];
        let signed = RawTransmission {
            authenticator: &auth,
            service_signature: Some(&signature),
            session_id: b"tls",
            correlation_id: b"c",
            entity_id: b"q",
            command: b"SUB",
        };
        let wire = encode_transmission(signed, true, true).unwrap();
        assert_eq!(&wire[..3], &[64, 9, 9]);
        assert_eq!(&wire[65..68], &[b'1', 64, 8]);
        assert_eq!(parse_transmission(&wire, b"tls", true, true), Ok(signed));
        assert_eq!(
            encode_transmission(signed, true, false),
            Err(SmpWireError::InvalidAuthentication)
        );
    }

    #[test]
    fn service_subscription_binds_both_keys_and_certificate_hash() {
        for entity_private in [
            PKey::generate_ed25519().unwrap(),
            PKey::generate_ed448().unwrap(),
        ] {
            let service_private = PKey::generate_ed25519().unwrap();
            let entity_public =
                PKey::public_key_from_der(&entity_private.public_key_to_der().unwrap()).unwrap();
            let service_public =
                PKey::public_key_from_der(&service_private.public_key_to_der().unwrap()).unwrap();
            let fingerprint = [7u8; 32];
            let wire = ServiceSubscriptionSigner {
                entity_key: &entity_private,
                service_key: &service_private,
                service_cert_hash: &fingerprint,
            }
            .sign(b"tls-session", b"corr", b"queue")
            .unwrap();
            let parsed = parse_transmission(&wire, b"tls-session", true, true).unwrap();
            assert_eq!(
                verify_service_subscription(parsed, &entity_public, &service_public, &fingerprint),
                Ok(true)
            );
            assert_eq!(
                verify_service_subscription(parsed, &entity_public, &service_public, &[0u8; 32]),
                Ok(false)
            );
            let mut changed = parsed;
            changed.command = b"GET";
            assert_eq!(
                verify_service_subscription(changed, &entity_public, &service_public, &fingerprint),
                Err(SmpWireError::InvalidAuthentication)
            );
            changed = parsed;
            changed.correlation_id = b"other";
            assert_eq!(
                verify_service_subscription(changed, &entity_public, &service_public, &fingerprint),
                Ok(false)
            );
            changed = parsed;
            changed.service_signature = None;
            assert_eq!(
                verify_service_subscription(changed, &entity_public, &service_public, &fingerprint),
                Err(SmpWireError::InvalidAuthentication)
            );
        }
    }

    #[test]
    fn x25519_authenticator_matches_both_dh_sides_and_binds_contents() {
        let client_private = PKey::generate_x25519().unwrap();
        let server_private = PKey::generate_x25519().unwrap();
        let client_public =
            PKey::public_key_from_der(&client_private.public_key_to_der().unwrap()).unwrap();
        let server_public =
            PKey::public_key_from_der(&server_private.public_key_to_der().unwrap()).unwrap();
        let correlation_id = b"short";
        let wire = X25519TransmissionAuthenticator {
            private_key: &client_private,
            peer_key: &server_public,
            imply_session_id: true,
            service_auth: true,
        }
        .authenticate(b"tls-session", correlation_id, b"queue", ClientCommand::Get)
        .unwrap();
        let parsed = parse_transmission(&wire, b"tls-session", true, true).unwrap();
        assert_eq!(parsed.authenticator.len(), 80);
        assert_eq!(
            verify_x25519_authenticator(parsed, &server_private, &client_public),
            Ok(true)
        );
        let mut altered = parsed;
        altered.command = b"ACK \x01x";
        assert_eq!(
            verify_x25519_authenticator(altered, &server_private, &client_public),
            Ok(false)
        );
        altered = parsed;
        altered.correlation_id = b"other";
        assert_eq!(
            verify_x25519_authenticator(altered, &server_private, &client_public),
            Ok(false)
        );
        let wrong_key = PKey::generate_x25519().unwrap();
        assert_eq!(
            verify_x25519_authenticator(parsed, &wrong_key, &client_public),
            Ok(false)
        );
    }
}
