//! In-memory routing of SMP responses and asynchronous server events.
//!
//! This mirrors the pinned client's correlation and entity-ID checks. It does
//! not persist queue state or interpret encrypted message bodies.

use std::collections::HashMap;

use thiserror::Error;

use crate::smp::OwnedTransmission;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRequest {
    pub entity_id: Vec<u8>,
    pub command: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RoutedTransmission {
    Response {
        request: PendingRequest,
        transmission: OwnedTransmission,
    },
    Event(OwnedTransmission),
    Unexpected(OwnedTransmission),
    EntityMismatch {
        request: PendingRequest,
        transmission: OwnedTransmission,
    },
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RouterError {
    #[error("SMP correlation ID must not be empty")]
    EmptyCorrelationId,
    #[error("SMP correlation ID is already pending")]
    DuplicateCorrelationId,
}

#[derive(Default)]
pub struct ResponseRouter {
    pending: HashMap<Vec<u8>, PendingRequest>,
}

impl ResponseRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register before sending the encoded command. Cancel the ID if sending
    /// fails or the request times out.
    pub fn register(
        &mut self,
        correlation_id: &[u8],
        entity_id: &[u8],
        command: &[u8],
    ) -> Result<(), RouterError> {
        if correlation_id.is_empty() {
            return Err(RouterError::EmptyCorrelationId);
        }
        if self.pending.contains_key(correlation_id) {
            return Err(RouterError::DuplicateCorrelationId);
        }
        self.pending.insert(
            correlation_id.to_vec(),
            PendingRequest {
                entity_id: entity_id.to_vec(),
                command: command.to_vec(),
            },
        );
        Ok(())
    }

    pub fn cancel(&mut self, correlation_id: &[u8]) -> Option<PendingRequest> {
        self.pending.remove(correlation_id)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// An empty correlation ID is an event. A nonempty ID is a response only
    /// when it matches a pending request and the request's entity ID.
    pub fn route(&mut self, transmission: OwnedTransmission) -> RoutedTransmission {
        if transmission.correlation_id.is_empty() {
            return RoutedTransmission::Event(transmission);
        }
        match self.pending.remove(&transmission.correlation_id) {
            Some(request) if request.entity_id == transmission.entity_id => {
                RoutedTransmission::Response {
                    request,
                    transmission,
                }
            }
            Some(request) => RoutedTransmission::EntityMismatch {
                request,
                transmission,
            },
            None => RoutedTransmission::Unexpected(transmission),
        }
    }

    pub fn route_batch(
        &mut self,
        transmissions: impl IntoIterator<Item = OwnedTransmission>,
    ) -> Vec<RoutedTransmission> {
        transmissions
            .into_iter()
            .map(|transmission| self.route(transmission))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transmission(correlation_id: &[u8], entity_id: &[u8], command: &[u8]) -> OwnedTransmission {
        OwnedTransmission {
            authenticator: vec![],
            service_signature: None,
            session_id: b"tls".to_vec(),
            correlation_id: correlation_id.to_vec(),
            entity_id: entity_id.to_vec(),
            command: command.to_vec(),
        }
    }

    #[test]
    fn routes_out_of_order_responses_and_async_events() {
        let mut router = ResponseRouter::new();
        router.register(b"a", b"queue-a", b"GET").unwrap();
        router.register(b"b", b"queue-b", b"SUB").unwrap();
        let routed = router.route_batch([
            transmission(b"", b"queue-a", b"MSG \x01xbody"),
            transmission(b"b", b"queue-b", b"OK"),
            transmission(b"a", b"queue-a", b"MSG \x01xbody"),
        ]);
        assert!(matches!(&routed[0], RoutedTransmission::Event(_)));
        assert!(
            matches!(&routed[1], RoutedTransmission::Response { request, .. } if request.command == b"SUB")
        );
        assert!(
            matches!(&routed[2], RoutedTransmission::Response { request, .. } if request.command == b"GET")
        );
        assert_eq!(router.pending_count(), 0);
    }

    #[test]
    fn rejects_duplicate_ids_and_mismatched_entities() {
        let mut router = ResponseRouter::new();
        assert_eq!(
            router.register(b"", b"queue", b"GET"),
            Err(RouterError::EmptyCorrelationId)
        );
        router.register(b"id", b"queue", b"GET").unwrap();
        assert_eq!(
            router.register(b"id", b"other", b"SUB"),
            Err(RouterError::DuplicateCorrelationId)
        );
        assert!(matches!(
            router.route(transmission(b"id", b"other", b"OK")),
            RoutedTransmission::EntityMismatch { .. }
        ));
        assert!(matches!(
            router.route(transmission(b"id", b"queue", b"OK")),
            RoutedTransmission::Unexpected(_)
        ));
        router.register(b"later", b"queue", b"GET").unwrap();
        assert!(router.cancel(b"later").is_some());
        assert!(matches!(
            router.route(transmission(b"later", b"queue", b"OK")),
            RoutedTransmission::Unexpected(_)
        ));
    }
}
