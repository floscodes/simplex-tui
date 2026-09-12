//! Forward-secret SMP v11+ transport block encryption.

use std::io::{self, Read, Write};

use crypto_secretbox::{
    Key, Nonce, XSalsa20Poly1305,
    aead::{Aead, KeyInit},
};
use hkdf::Hkdf;
use sha2::Sha512;
use thiserror::Error;

use crate::transport::{SMP_BLOCK_SIZE, pad_block, unpad_block};

const TAG_SIZE: usize = 16;
const PADDED_SIZE: usize = SMP_BLOCK_SIZE - TAG_SIZE;

#[derive(Debug, Error)]
pub enum EncryptedBlockError {
    #[error("SMP key agreement produced an invalid secret")]
    InvalidSecret,
    #[error("SMP message is too large for an encrypted block")]
    TooLarge,
    #[error("encrypted SMP block has an invalid size")]
    InvalidSize,
    #[error("encrypted SMP block authentication failed")]
    AuthenticationFailed,
    #[error("decrypted SMP block has an invalid length")]
    InvalidPadding,
}

struct ChainKey([u8; 32]);

impl ChainKey {
    fn advance(&mut self) -> ([u8; 32], [u8; 24]) {
        let hkdf = Hkdf::<Sha512>::new(Some(b""), &self.0);
        let mut output = [0u8; 88];
        hkdf.expand(b"SimpleXSbChain", &mut output)
            .expect("88 bytes is within HKDF-SHA512 output limit");
        self.0.copy_from_slice(&output[..32]);
        let mut key = [0u8; 32];
        key.copy_from_slice(&output[32..64]);
        let mut nonce = [0u8; 24];
        nonce.copy_from_slice(&output[64..88]);
        (key, nonce)
    }
}

/// Client-side send and receive chains. A failed authentication invalidates
/// the connection; callers must close it rather than try another block.
pub struct SmpBlockCipher {
    send: ChainKey,
    receive: ChainKey,
}

impl SmpBlockCipher {
    pub fn for_client(
        session_id: &[u8],
        shared_secret: &[u8],
    ) -> Result<Self, EncryptedBlockError> {
        let (server_send, client_send) = initial_chains(session_id, shared_secret)?;
        Ok(Self {
            send: ChainKey(client_send),
            receive: ChainKey(server_send),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_server(
        session_id: &[u8],
        shared_secret: &[u8],
    ) -> Result<Self, EncryptedBlockError> {
        let (server_send, client_send) = initial_chains(session_id, shared_secret)?;
        Ok(Self {
            send: ChainKey(server_send),
            receive: ChainKey(client_send),
        })
    }

    /// Encrypt and pad one message to an SMP transport block.
    pub fn seal(&mut self, message: &[u8]) -> Result<Vec<u8>, EncryptedBlockError> {
        let padded = pad_block(message, PADDED_SIZE).map_err(|_| EncryptedBlockError::TooLarge)?;
        let (key, nonce) = self.send.advance();
        XSalsa20Poly1305::new(Key::from_slice(&key))
            .encrypt(Nonce::from_slice(&nonce), padded.as_ref())
            .map_err(|_| EncryptedBlockError::AuthenticationFailed)
    }

    /// Authenticate and decrypt one fixed-size SMP transport block.
    pub fn open(&mut self, block: &[u8]) -> Result<Vec<u8>, EncryptedBlockError> {
        if block.len() != SMP_BLOCK_SIZE {
            return Err(EncryptedBlockError::InvalidSize);
        }
        let (key, nonce) = self.receive.advance();
        let padded = XSalsa20Poly1305::new(Key::from_slice(&key))
            .decrypt(Nonce::from_slice(&nonce), block)
            .map_err(|_| EncryptedBlockError::AuthenticationFailed)?;
        unpad_block(&padded)
            .map(<[u8]>::to_vec)
            .map_err(|_| EncryptedBlockError::InvalidPadding)
    }

    pub fn write_block(&mut self, stream: &mut impl Write, message: &[u8]) -> io::Result<()> {
        let block = self
            .seal(message)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        stream.write_all(&block)
    }

    pub fn read_block(&mut self, stream: &mut impl Read) -> io::Result<Vec<u8>> {
        let mut block = [0u8; SMP_BLOCK_SIZE];
        stream.read_exact(&mut block)?;
        self.open(&block)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

fn initial_chains(
    session_id: &[u8],
    shared_secret: &[u8],
) -> Result<([u8; 32], [u8; 32]), EncryptedBlockError> {
    if shared_secret.len() != 32 && shared_secret.len() != 56 {
        return Err(EncryptedBlockError::InvalidSecret);
    }
    let hkdf = Hkdf::<Sha512>::new(Some(session_id), shared_secret);
    let mut output = [0u8; 64];
    hkdf.expand(b"SimpleXSbChainInit", &mut output)
        .expect("64 bytes is within HKDF-SHA512 output limit");
    let mut server_send = [0u8; 32];
    server_send.copy_from_slice(&output[..32]);
    let mut client_send = [0u8; 32];
    client_send.copy_from_slice(&output[32..]);
    Ok((server_send, client_send))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_and_server_exchange_multiple_blocks() {
        let mut client = SmpBlockCipher::for_client(b"tls-finished", &[7; 32]).unwrap();
        let mut server = SmpBlockCipher::for_server(b"tls-finished", &[7; 32]).unwrap();
        for message in [b"first".as_slice(), b"second", b""] {
            let block = client.seal(message).unwrap();
            assert_eq!(block.len(), SMP_BLOCK_SIZE);
            assert_eq!(server.open(&block).unwrap(), message);
        }
        let reply = server.seal(b"reply").unwrap();
        assert_eq!(client.open(&reply).unwrap(), b"reply");
    }

    #[test]
    fn rejects_tampering_wrong_size_and_wrong_session() {
        let mut client = SmpBlockCipher::for_client(b"session", &[9; 32]).unwrap();
        let mut server = SmpBlockCipher::for_server(b"session", &[9; 32]).unwrap();
        let mut block = client.seal(b"secret").unwrap();
        block[100] ^= 1;
        assert!(matches!(
            server.open(&block),
            Err(EncryptedBlockError::AuthenticationFailed)
        ));
        assert!(matches!(
            server.open(&block[..100]),
            Err(EncryptedBlockError::InvalidSize)
        ));
        let mut wrong_session = SmpBlockCipher::for_server(b"other", &[9; 32]).unwrap();
        assert!(matches!(
            wrong_session.open(&client.seal(b"new").unwrap()),
            Err(EncryptedBlockError::AuthenticationFailed)
        ));
    }
}
