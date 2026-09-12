//! SMP transport block framing from the pinned `simplexmq` implementation.
//!
//! This handles the plaintext framing used before block encryption. It does
//! not establish a TLS connection or implement the SMP messaging agent.

use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use thiserror::Error;

pub const SMP_BLOCK_SIZE: usize = 16_384;
const MAX_MESSAGE_LEN: usize = u16::MAX as usize - 2;
pub const MIN_CLIENT_SMP_VERSION: u16 = 6;
pub const CURRENT_CLIENT_SMP_VERSION: u16 = 20;
const PROXY_SERVER_HANDSHAKE_VERSION: u16 = 14;
const DELETED_EVENT_VERSION: u16 = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmpVersionRange {
    pub min: u16,
    pub max: u16,
}

impl SmpVersionRange {
    pub const CLIENT_SUPPORTED: Self = Self {
        min: MIN_CLIENT_SMP_VERSION,
        max: CURRENT_CLIENT_SMP_VERSION,
    };

    pub fn new(min: u16, max: u16) -> Option<Self> {
        (min > 0 && min <= max).then_some(Self { min, max })
    }

    /// `VersionRange` is two network-order Word16 values in the SMP binary
    /// encoding. A handshake parser can consume this four-byte prefix.
    pub fn encode(self) -> [u8; 4] {
        let [min_high, min_low] = self.min.to_be_bytes();
        let [max_high, max_low] = self.max.to_be_bytes();
        [min_high, min_low, max_high, max_low]
    }

    pub fn decode(prefix: &[u8]) -> Option<Self> {
        let [min_high, min_low, max_high, max_low] = prefix else {
            return None;
        };
        Self::new(
            u16::from_be_bytes([*min_high, *min_low]),
            u16::from_be_bytes([*max_high, *max_low]),
        )
    }

    /// Return the highest mutually supported version. When acting as an SMP
    /// proxy for a server below v14, match the upstream v10 downgrade rule.
    pub fn negotiate_client(self, server: Self, proxy_server: bool) -> Option<u16> {
        let client_max = if proxy_server && server.max < PROXY_SERVER_HANDSHAKE_VERSION {
            self.min.max(DELETED_EVENT_VERSION)
        } else {
            self.max
        };
        let min = self.min.max(server.min);
        let max = client_max.min(server.max);
        (min <= max).then_some(max)
    }
}

/// Parsed, but unauthenticated, prefix of `SMPServerHandshake`. The remaining
/// bytes contain an optional signed certificate/public key and must be
/// verified before any client handshake response is sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerHello<'a> {
    pub versions: SmpVersionRange,
    pub session_id: &'a [u8],
    pub authentication: &'a [u8],
}

/// Raw certificate chain and signed key from `CertChainPubKey`. These are
/// untrusted DER bytes until the chain and signature are verified.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnverifiedServerAuth<'a> {
    pub certificates: Vec<&'a [u8]>,
    pub signed_key: &'a [u8],
    pub extensions: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ServerHelloError {
    #[error("server handshake is truncated")]
    Truncated,
    #[error("server advertised an invalid SMP version range")]
    InvalidVersionRange,
    #[error("server handshake session does not match TLS channel binding")]
    SessionMismatch,
    #[error("server and client have no compatible SMP version")]
    IncompatibleVersion,
    #[error("server authentication is missing")]
    MissingAuthentication,
    #[error("server authentication is malformed")]
    MalformedAuthentication,
    #[error("server certificate chain has an unsupported length")]
    InvalidCertificateChain,
    #[error("handshake certificate chain does not match the TLS peer chain")]
    CertificateChainMismatch,
    #[error("server identity certificate fingerprint does not match")]
    IdentityMismatch,
}

impl UnverifiedServerAuth<'_> {
    /// Bind raw handshake certificates to a chain already validated by TLS,
    /// and compare the pinned SHA-256 identity fingerprint. This is not a
    /// substitute for checking the signed key's signature.
    pub fn check_identity_binding(
        &self,
        tls_peer_certificates: &[&[u8]],
        expected_fingerprint: &[u8; 32],
    ) -> Result<(), ServerHelloError> {
        if !(2..=4).contains(&self.certificates.len()) {
            return Err(ServerHelloError::InvalidCertificateChain);
        }
        if self.certificates.as_slice() != tls_peer_certificates {
            return Err(ServerHelloError::CertificateChainMismatch);
        }
        let actual_fingerprint = Sha256::digest(self.certificates[1]);
        if actual_fingerprint.as_slice() != expected_fingerprint {
            return Err(ServerHelloError::IdentityMismatch);
        }
        Ok(())
    }
}

impl<'a> ServerHello<'a> {
    /// Parse the version range and length-prefixed TLS-unique session ID.
    /// Certificate bytes remain opaque until a cryptographic verifier exists.
    pub fn parse(input: &'a [u8]) -> Result<Self, ServerHelloError> {
        let version_bytes = input.get(..4).ok_or(ServerHelloError::Truncated)?;
        let versions =
            SmpVersionRange::decode(version_bytes).ok_or(ServerHelloError::InvalidVersionRange)?;
        let session_len = *input.get(4).ok_or(ServerHelloError::Truncated)? as usize;
        let session_end = 5 + session_len;
        let session_id = input
            .get(5..session_end)
            .ok_or(ServerHelloError::Truncated)?;
        let authentication = &input[session_end..];
        Ok(Self {
            versions,
            session_id,
            authentication,
        })
    }

    /// Check the TLS channel binding and choose a compatible version. This
    /// does not authenticate the server certificate.
    pub fn select_version(
        self,
        tls_unique: &[u8],
        client: SmpVersionRange,
        proxy_server: bool,
    ) -> Result<u16, ServerHelloError> {
        if self.session_id != tls_unique {
            return Err(ServerHelloError::SessionMismatch);
        }
        client
            .negotiate_client(self.versions, proxy_server)
            .ok_or(ServerHelloError::IncompatibleVersion)
    }

    /// Parse the optional v7+ `CertChainPubKey` layout: a nonempty list of
    /// `Large` DER certificates followed by one `Large` signed key. This
    /// method does not perform X.509 or signature verification.
    pub fn unverified_authentication(self) -> Result<UnverifiedServerAuth<'a>, ServerHelloError> {
        if self.versions.max < 7 || self.authentication.is_empty() {
            return Err(ServerHelloError::MissingAuthentication);
        }
        let (&count, mut remaining) = self
            .authentication
            .split_first()
            .ok_or(ServerHelloError::MissingAuthentication)?;
        if count == 0 {
            return Err(ServerHelloError::MalformedAuthentication);
        }
        let mut certificates = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let (certificate, tail) = read_large(remaining)?;
            if certificate.is_empty() {
                return Err(ServerHelloError::MalformedAuthentication);
            }
            certificates.push(certificate);
            remaining = tail;
        }
        let (signed_key, extensions) = read_large(remaining)?;
        if signed_key.is_empty() {
            return Err(ServerHelloError::MalformedAuthentication);
        }
        Ok(UnverifiedServerAuth {
            certificates,
            signed_key,
            extensions,
        })
    }
}

fn read_large(input: &[u8]) -> Result<(&[u8], &[u8]), ServerHelloError> {
    let [high, low, rest @ ..] = input else {
        return Err(ServerHelloError::MalformedAuthentication);
    };
    let length = u16::from_be_bytes([*high, *low]) as usize;
    let payload = rest
        .get(..length)
        .ok_or(ServerHelloError::MalformedAuthentication)?;
    Ok((payload, &rest[length..]))
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BlockError {
    #[error("message does not fit in the transport block")]
    TooLarge,
    #[error("transport block has an invalid length")]
    InvalidLength,
}

/// Match `Simplex.Messaging.Crypto.pad`: a big-endian 16-bit length, body,
/// then `#` padding to the negotiated block size.
pub fn pad_block(message: &[u8], block_size: usize) -> Result<Vec<u8>, BlockError> {
    if message.len() > MAX_MESSAGE_LEN || block_size < 2 || message.len() > block_size - 2 {
        return Err(BlockError::TooLarge);
    }
    let mut block = Vec::with_capacity(block_size);
    block.extend_from_slice(&(message.len() as u16).to_be_bytes());
    block.extend_from_slice(message);
    block.resize(block_size, b'#');
    Ok(block)
}

/// Match `Simplex.Messaging.Crypto.unPad`: padding bytes are ignored after
/// the length-delimited body, because encrypted blocks need no marker check.
pub fn unpad_block(block: &[u8]) -> Result<&[u8], BlockError> {
    let (&high, rest) = block.split_first().ok_or(BlockError::InvalidLength)?;
    let (&low, body) = rest.split_first().ok_or(BlockError::InvalidLength)?;
    let message_len = u16::from_be_bytes([high, low]) as usize;
    body.get(..message_len).ok_or(BlockError::InvalidLength)
}

/// Frame one unencrypted SMP transport block with the upstream block size.
pub fn pad_smp_block(message: &[u8]) -> Result<Vec<u8>, BlockError> {
    pad_block(message, SMP_BLOCK_SIZE)
}

/// Decode one complete unencrypted SMP transport block.
pub fn unpad_smp_block(block: &[u8]) -> Result<&[u8], BlockError> {
    if block.len() != SMP_BLOCK_SIZE {
        return Err(BlockError::InvalidLength);
    }
    unpad_block(block)
}

/// Write exactly one plaintext SMP block to an established byte stream.
pub fn write_smp_block(stream: &mut impl Write, message: &[u8]) -> io::Result<()> {
    let block = pad_smp_block(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    stream.write_all(&block)
}

/// Read exactly one plaintext SMP block from an established byte stream.
pub fn read_smp_block(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut block = [0; SMP_BLOCK_SIZE];
    stream.read_exact(&mut block)?;
    unpad_smp_block(&block)
        .map(<[u8]>::to_vec)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn frames_smp_block() {
        let block = pad_smp_block(b"hello").unwrap();
        assert_eq!(block.len(), SMP_BLOCK_SIZE);
        assert_eq!(&block[..7], b"\0\x05hello");
        assert!(block[7..].iter().all(|byte| *byte == b'#'));
        assert_eq!(unpad_smp_block(&block), Ok(&b"hello"[..]));
    }

    #[test]
    fn negotiates_smp_versions_and_proxy_downgrade() {
        let client = SmpVersionRange::CLIENT_SUPPORTED;
        assert_eq!(
            client.negotiate_client(SmpVersionRange::new(6, 20).unwrap(), false),
            Some(20)
        );
        assert_eq!(
            client.negotiate_client(SmpVersionRange::new(6, 12).unwrap(), true),
            Some(10)
        );
        assert_eq!(
            client.negotiate_client(SmpVersionRange::new(6, 14).unwrap(), true),
            Some(14)
        );
        assert_eq!(
            client.negotiate_client(SmpVersionRange::new(1, 5).unwrap(), false),
            None
        );
        assert_eq!(SmpVersionRange::new(9, 8), None);
        assert_eq!(client.encode(), [0, 6, 0, 20]);
        assert_eq!(SmpVersionRange::decode(&[0, 6, 0, 20]), Some(client));
        assert_eq!(SmpVersionRange::decode(&[0, 20, 0, 6]), None);
        assert_eq!(SmpVersionRange::decode(&[0, 6, 0]), None);
    }

    #[test]
    fn parses_server_hello_prefix_and_checks_channel_binding() {
        let bytes = [0, 6, 0, 20, 3, 1, 2, 3, 0xaa, 0xbb];
        let hello = ServerHello::parse(&bytes).unwrap();
        assert_eq!(hello.versions, SmpVersionRange::CLIENT_SUPPORTED);
        assert_eq!(hello.session_id, &[1, 2, 3]);
        assert_eq!(hello.authentication, &[0xaa, 0xbb]);
        assert_eq!(
            hello.select_version(&[1, 2, 3], SmpVersionRange::CLIENT_SUPPORTED, false),
            Ok(20)
        );
        assert_eq!(
            hello.select_version(&[1, 2], SmpVersionRange::CLIENT_SUPPORTED, false),
            Err(ServerHelloError::SessionMismatch)
        );
        assert_eq!(
            ServerHello::parse(&bytes[..7]),
            Err(ServerHelloError::Truncated)
        );
        assert_eq!(
            ServerHello::parse(&[0, 20, 0, 6, 0]),
            Err(ServerHelloError::InvalidVersionRange)
        );
    }

    #[test]
    fn parses_unverified_certificate_chain_layout() {
        let bytes = [
            0, 6, 0, 20, 1, 9, // version range and TLS session
            2, // two certificates
            0, 2, 0x30, 1, // first DER certificate
            0, 2, 0x30, 2, // second DER certificate
            0, 2, 0x30, 3,    // signed key DER
            0xaa, // future extension
        ];
        let hello = ServerHello::parse(&bytes).unwrap();
        let auth = hello.unverified_authentication().unwrap();
        assert_eq!(auth.certificates, vec![&[0x30, 1][..], &[0x30, 2][..]]);
        assert_eq!(auth.signed_key, &[0x30, 3]);
        assert_eq!(auth.extensions, &[0xaa]);
        let fingerprint: [u8; 32] = Sha256::digest(auth.certificates[1]).into();
        assert_eq!(
            auth.check_identity_binding(&[&[0x30, 1], &[0x30, 2]], &fingerprint),
            Ok(())
        );
        assert_eq!(
            auth.check_identity_binding(&[&[0x30, 1], &[0x30, 4]], &fingerprint),
            Err(ServerHelloError::CertificateChainMismatch)
        );
        assert_eq!(
            auth.check_identity_binding(&[&[0x30, 1], &[0x30, 2]], &[0; 32]),
            Err(ServerHelloError::IdentityMismatch)
        );
        assert_eq!(
            ServerHello::parse(&bytes[..bytes.len() - 3])
                .unwrap()
                .unverified_authentication(),
            Err(ServerHelloError::MalformedAuthentication)
        );
        assert_eq!(
            ServerHello::parse(&[0, 6, 0, 20, 0, 0])
                .unwrap()
                .unverified_authentication(),
            Err(ServerHelloError::MalformedAuthentication)
        );
    }

    #[test]
    fn honors_length_and_size_bounds() {
        assert_eq!(pad_block(b"abcd", 5), Err(BlockError::TooLarge));
        assert_eq!(pad_block(b"", 1), Err(BlockError::TooLarge));
        assert_eq!(unpad_block(&[0, 3, b'a']), Err(BlockError::InvalidLength));
        assert_eq!(unpad_block(&[0]), Err(BlockError::InvalidLength));
        assert_eq!(unpad_smp_block(&[0, 0]), Err(BlockError::InvalidLength));
        assert_eq!(unpad_block(&[0, 1, b'x', b'!']), Ok(&b"x"[..]));
    }

    #[test]
    fn reads_consecutive_blocks_and_reports_eof() {
        let mut wire = Vec::new();
        write_smp_block(&mut wire, b"one").unwrap();
        write_smp_block(&mut wire, b"two").unwrap();
        let mut stream = Cursor::new(wire);
        assert_eq!(read_smp_block(&mut stream).unwrap(), b"one");
        assert_eq!(read_smp_block(&mut stream).unwrap(), b"two");
        assert_eq!(
            read_smp_block(&mut stream).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
    }
}
