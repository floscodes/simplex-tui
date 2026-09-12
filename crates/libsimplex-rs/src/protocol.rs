//! Rust implementations of self-contained SimpleX chat wire formats.
//!
//! The inline file chunk format comes from `Simplex.Chat.Protocol` in the
//! pinned upstream source. It is independent of the Haskell controller ABI.

use thiserror::Error;

/// Chat protocol versions supported by the pinned SimpleX Chat source.
pub const INITIAL_CHAT_VERSION: u16 = 1;
pub const CURRENT_CHAT_VERSION: u16 = 19;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChatVersionRange {
    pub min: u16,
    pub max: u16,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ChatVersionRangeError {
    #[error("chat version range must start at version 1 or higher")]
    ZeroVersion,
    #[error("chat version range minimum exceeds its maximum")]
    Reversed,
}

impl ChatVersionRange {
    pub const SUPPORTED: Self = Self {
        min: INITIAL_CHAT_VERSION,
        max: CURRENT_CHAT_VERSION,
    };

    pub fn new(min: u16, max: u16) -> Result<Self, ChatVersionRangeError> {
        if min == 0 {
            return Err(ChatVersionRangeError::ZeroVersion);
        }
        if min > max {
            return Err(ChatVersionRangeError::Reversed);
        }
        Ok(Self { min, max })
    }

    /// Match `peerConnChatVersion` in the pinned `Simplex.Chat.Types`.
    /// A disjoint range selects the closest peer boundary, as upstream does.
    pub fn peer_connection_version(self, peer: Self) -> u16 {
        if self.min <= peer.max && peer.min <= self.max {
            self.max.min(peer.max)
        } else if peer.min > self.max {
            peer.min
        } else {
            peer.max
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InlineFileChunk<'a> {
    Data { number: u8, bytes: &'a [u8] },
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InlineFileChunkError {
    #[error("inline file chunk is empty")]
    Empty,
    #[error("inline file chunk number 0 is reserved for cancellation")]
    ReservedNumber,
    #[error("cancellation chunk has unexpected trailing bytes")]
    TrailingBytes,
}

impl<'a> InlineFileChunk<'a> {
    /// Decode the complete wire payload. The first byte is the chunk number;
    /// zero denotes cancellation. Remaining bytes are the chunk body.
    pub fn decode(input: &'a [u8]) -> Result<Self, InlineFileChunkError> {
        let (&number, bytes) = input.split_first().ok_or(InlineFileChunkError::Empty)?;
        if number == 0 {
            if bytes.is_empty() {
                Ok(Self::Cancel)
            } else {
                Err(InlineFileChunkError::TrailingBytes)
            }
        } else {
            Ok(Self::Data { number, bytes })
        }
    }

    /// Encode using the `InlineFileChunk` format from SimpleX Chat.
    pub fn encode(&self) -> Result<Vec<u8>, InlineFileChunkError> {
        match self {
            Self::Cancel => Ok(vec![0]),
            Self::Data { number: 0, .. } => Err(InlineFileChunkError::ReservedNumber),
            Self::Data { number, bytes } => {
                let mut encoded = Vec::with_capacity(1 + bytes.len());
                encoded.push(*number);
                encoded.extend_from_slice(bytes);
                Ok(encoded)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_chat_version_like_upstream() {
        let local = ChatVersionRange::new(1, 19).unwrap();
        assert_eq!(
            local.peer_connection_version(ChatVersionRange::new(1, 8).unwrap()),
            8
        );
        assert_eq!(
            local.peer_connection_version(ChatVersionRange::new(5, 25).unwrap()),
            19
        );
        assert_eq!(
            local.peer_connection_version(ChatVersionRange::new(20, 25).unwrap()),
            20
        );
        assert_eq!(
            ChatVersionRange::new(10, 19)
                .unwrap()
                .peer_connection_version(ChatVersionRange::new(1, 9).unwrap()),
            9
        );
        assert_eq!(ChatVersionRange::SUPPORTED, local);
        assert_eq!(
            ChatVersionRange::new(0, 1),
            Err(ChatVersionRangeError::ZeroVersion)
        );
        assert_eq!(
            ChatVersionRange::new(3, 2),
            Err(ChatVersionRangeError::Reversed)
        );
    }

    #[test]
    fn inline_chunk_matches_upstream_wire_format() {
        let payload = [1, 0, 255, 42];
        let chunk = InlineFileChunk::decode(&payload).unwrap();
        assert_eq!(
            chunk,
            InlineFileChunk::Data {
                number: 1,
                bytes: &[0, 255, 42]
            }
        );
        assert_eq!(chunk.encode().unwrap(), payload);
        assert_eq!(
            InlineFileChunk::decode(&[0]).unwrap(),
            InlineFileChunk::Cancel
        );
        assert_eq!(InlineFileChunk::Cancel.encode().unwrap(), [0]);
    }

    #[test]
    fn rejects_ambiguous_or_malformed_chunks() {
        assert_eq!(
            InlineFileChunk::decode(&[]),
            Err(InlineFileChunkError::Empty)
        );
        assert_eq!(
            InlineFileChunk::decode(&[0, 1]),
            Err(InlineFileChunkError::TrailingBytes)
        );
        assert_eq!(
            InlineFileChunk::Data {
                number: 0,
                bytes: b"x"
            }
            .encode(),
            Err(InlineFileChunkError::ReservedNumber)
        );
    }
}
