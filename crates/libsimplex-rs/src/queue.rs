//! SMP `NEW` queue request and `IDS` response wire formats.
//!
//! This covers ordinary queues without short-link data or notification
//! credentials. Queue persistence and retries belong to the future agent.

use openssl::pkey::{Id, PKey, PKeyRef, Private, Public};

use crate::smp::{SmpWireError, put_short, sign_encoded_transmission, take_short};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionMode {
    Subscribe,
    CreateOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueMode {
    Messaging,
    Contact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotificationCredentials<'a> {
    pub notifier_id: &'a [u8],
    pub server_dh_key_der: &'a [u8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueIds<'a> {
    pub recipient_id: &'a [u8],
    pub sender_id: &'a [u8],
    pub server_dh_key_der: &'a [u8],
    pub mode: Option<QueueMode>,
    pub link_id: Option<&'a [u8]>,
    pub service_id: Option<&'a [u8]>,
    pub notification: Option<NotificationCredentials<'a>>,
}

fn encode_public_key(key: &PKeyRef<Public>, out: &mut Vec<u8>) -> Result<(), SmpWireError> {
    let der = key
        .public_key_to_der()
        .map_err(|_| SmpWireError::InvalidAuthentication)?;
    put_short(out, &der)
}

fn validate_dh_key(der: &[u8]) -> Result<(), SmpWireError> {
    let key = PKey::public_key_from_der(der).map_err(|_| SmpWireError::Malformed)?;
    if key.id() != Id::X25519 {
        return Err(SmpWireError::Malformed);
    }
    Ok(())
}

/// Encode the base `NEW` request for SMP v6–v20. The recipient authorization
/// key may be Ed25519, Ed448, or X25519 on v7+, while v6 only supports a
/// signature key. The returned bytes must be signed as a creator command with
/// an empty entity ID before sending.
pub fn encode_new_queue(
    version: u16,
    recipient_auth_key: &PKeyRef<Public>,
    recipient_dh_key: &PKeyRef<Public>,
    subscription: SubscriptionMode,
) -> Result<Vec<u8>, SmpWireError> {
    if !(6..=20).contains(&version)
        || !matches!(
            recipient_auth_key.id(),
            Id::ED25519 | Id::ED448 | Id::X25519
        )
        || (version < 7 && recipient_auth_key.id() == Id::X25519)
        || recipient_dh_key.id() != Id::X25519
    {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let mut out = b"NEW ".to_vec();
    encode_public_key(recipient_auth_key, &mut out)?;
    encode_public_key(recipient_dh_key, &mut out)?;
    if version >= 9 {
        out.push(b'0'); // no server basic auth
        out.push(match subscription {
            SubscriptionMode::Subscribe => b'S',
            SubscriptionMode::CreateOnly => b'C',
        });
        if version >= 15 {
            out.push(b'0'); // no QueueReqData
        } else {
            out.push(b'F'); // senderCanSecure Nothing
        }
        if version >= 17 {
            out.push(b'0'); // no notification credentials
        }
    } else {
        out.push(match subscription {
            SubscriptionMode::Subscribe => b'S',
            SubscriptionMode::CreateOnly => b'C',
        });
    }
    Ok(out)
}

/// Create a signed `NEW` transmission. The creator command must carry no
/// entity ID; its Ed25519/Ed448 key is included in the encoded request.
pub fn sign_new_queue(
    version: u16,
    session_id: &[u8],
    correlation_id: &[u8],
    recipient_auth_key: &PKeyRef<Private>,
    recipient_dh_key: &PKeyRef<Public>,
    subscription: SubscriptionMode,
) -> Result<Vec<u8>, SmpWireError> {
    if !matches!(recipient_auth_key.id(), Id::ED25519 | Id::ED448) {
        return Err(SmpWireError::InvalidAuthentication);
    }
    let public_der = recipient_auth_key
        .public_key_to_der()
        .map_err(|_| SmpWireError::InvalidAuthentication)?;
    let public =
        PKey::public_key_from_der(&public_der).map_err(|_| SmpWireError::InvalidAuthentication)?;
    let command = encode_new_queue(version, &public, recipient_dh_key, subscription)?;
    sign_encoded_transmission(
        session_id,
        correlation_id,
        b"",
        &command,
        recipient_auth_key,
        version >= 7,
        version >= 16,
    )
}

fn take_optional<'a, T>(
    input: &mut &'a [u8],
    parse: impl FnOnce(&mut &'a [u8]) -> Result<T, SmpWireError>,
) -> Result<Option<T>, SmpWireError> {
    let (&tag, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
    *input = rest;
    match tag {
        b'0' => Ok(None),
        b'1' => parse(input).map(Some),
        _ => Err(SmpWireError::Malformed),
    }
}

/// Parse the versioned `IDS` body, retaining raw DER and IDs for the agent.
/// The caller must verify that the containing transmission has no entity ID.
pub fn parse_queue_ids(version: u16, input: &[u8]) -> Result<QueueIds<'_>, SmpWireError> {
    if !(6..=20).contains(&version) {
        return Err(SmpWireError::Malformed);
    }
    let mut input = input
        .strip_prefix(b"IDS ")
        .ok_or(SmpWireError::UnknownTag)?;
    let recipient_id = take_short(&mut input)?;
    let sender_id = take_short(&mut input)?;
    let server_dh_key_der = take_short(&mut input)?;
    if recipient_id.is_empty() || sender_id.is_empty() {
        return Err(SmpWireError::Malformed);
    }
    validate_dh_key(server_dh_key_der)?;
    let mode = if version >= 15 {
        take_optional(&mut input, |input| {
            let (&mode, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
            *input = rest;
            match mode {
                b'M' => Ok(QueueMode::Messaging),
                b'C' => Ok(QueueMode::Contact),
                _ => Err(SmpWireError::Malformed),
            }
        })?
    } else if version >= 9 {
        let (&secure, rest) = input.split_first().ok_or(SmpWireError::Malformed)?;
        input = rest;
        Some(match secure {
            b'T' => QueueMode::Messaging,
            b'F' => QueueMode::Contact,
            _ => return Err(SmpWireError::Malformed),
        })
    } else {
        None
    };
    let link_id = if version >= 15 {
        take_optional(&mut input, take_short)?
    } else {
        None
    };
    let service_id = if version >= 16 {
        take_optional(&mut input, take_short)?
    } else {
        None
    };
    let notification = if version >= 17 {
        take_optional(&mut input, |input| {
            let notifier_id = take_short(input)?;
            let server_dh_key_der = take_short(input)?;
            validate_dh_key(server_dh_key_der)?;
            Ok(NotificationCredentials {
                notifier_id,
                server_dh_key_der,
            })
        })?
    } else {
        None
    };
    if !input.is_empty() {
        return Err(SmpWireError::Malformed);
    }
    Ok(QueueIds {
        recipient_id,
        sender_id,
        server_dh_key_der,
        mode,
        link_id,
        service_id,
        notification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smp::{parse_transmission, verify_transmission_signature};

    #[test]
    fn new_queue_version_boundaries_match_upstream_layout() {
        let auth = PKey::generate_ed25519().unwrap();
        let auth = PKey::public_key_from_der(&auth.public_key_to_der().unwrap()).unwrap();
        let dh = PKey::generate_x25519().unwrap();
        let dh = PKey::public_key_from_der(&dh.public_key_to_der().unwrap()).unwrap();
        let mut prefix = b"NEW ".to_vec();
        put_short(&mut prefix, &auth.public_key_to_der().unwrap()).unwrap();
        put_short(&mut prefix, &dh.public_key_to_der().unwrap()).unwrap();
        for (version, suffix) in [
            (6, b"S".as_slice()),
            (9, b"0SF"),
            (15, b"0S0"),
            (17, b"0S00"),
            (20, b"0S00"),
        ] {
            let mut expected = prefix.clone();
            expected.extend_from_slice(suffix);
            assert_eq!(
                encode_new_queue(version, &auth, &dh, SubscriptionMode::Subscribe),
                Ok(expected)
            );
        }
        assert_eq!(
            encode_new_queue(21, &auth, &dh, SubscriptionMode::Subscribe),
            Err(SmpWireError::InvalidAuthentication)
        );
    }

    #[test]
    fn ids_response_parses_optional_versioned_fields() {
        let dh = PKey::generate_x25519().unwrap();
        let der = dh.public_key_to_der().unwrap();
        let mut base = b"IDS ".to_vec();
        put_short(&mut base, b"recipient").unwrap();
        put_short(&mut base, b"sender").unwrap();
        put_short(&mut base, &der).unwrap();
        let mut v20 = base.clone();
        v20.extend_from_slice(b"1M");
        v20.extend_from_slice(b"0"); // link ID
        v20.extend_from_slice(b"1\x03svc");
        v20.extend_from_slice(b"0"); // notification credentials
        let parsed = parse_queue_ids(20, &v20).unwrap();
        assert_eq!(parsed.recipient_id, b"recipient");
        assert_eq!(parsed.sender_id, b"sender");
        assert_eq!(parsed.mode, Some(QueueMode::Messaging));
        assert_eq!(parsed.service_id, Some(b"svc".as_slice()));
        assert_eq!(parsed.link_id, None);
        assert_eq!(parsed.notification, None);
        let mut notified = base.clone();
        notified.extend_from_slice(b"0001");
        put_short(&mut notified, b"notifier").unwrap();
        put_short(&mut notified, &der).unwrap();
        let parsed = parse_queue_ids(20, &notified).unwrap();
        assert_eq!(parsed.notification.unwrap().notifier_id, b"notifier");
        let mut v9 = base.clone();
        v9.push(b'T');
        assert_eq!(
            parse_queue_ids(9, &v9).unwrap().mode,
            Some(QueueMode::Messaging)
        );
        assert!(parse_queue_ids(20, &base).is_err());
        v20.push(b'!');
        assert_eq!(parse_queue_ids(20, &v20), Err(SmpWireError::Malformed));
        let mut bad_key = b"IDS ".to_vec();
        put_short(&mut bad_key, b"recipient").unwrap();
        put_short(&mut bad_key, b"sender").unwrap();
        put_short(&mut bad_key, b"not-der").unwrap();
        bad_key.extend_from_slice(b"0000");
        assert_eq!(parse_queue_ids(20, &bad_key), Err(SmpWireError::Malformed));
    }

    #[test]
    fn new_queue_transmission_is_signed_with_empty_entity_id() {
        let auth = PKey::generate_ed25519().unwrap();
        let public = PKey::public_key_from_der(&auth.public_key_to_der().unwrap()).unwrap();
        let dh = PKey::generate_x25519().unwrap();
        let dh = PKey::public_key_from_der(&dh.public_key_to_der().unwrap()).unwrap();
        let wire = sign_new_queue(
            20,
            b"tls",
            b"corr",
            &auth,
            &dh,
            SubscriptionMode::CreateOnly,
        )
        .unwrap();
        let parsed = parse_transmission(&wire, b"tls", true, true).unwrap();
        assert_eq!(parsed.entity_id, b"");
        assert_eq!(parsed.correlation_id, b"corr");
        assert!(parsed.command.starts_with(b"NEW "));
        assert_eq!(parsed.command.last(), Some(&b'0'));
        assert_eq!(verify_transmission_signature(parsed, &public), Ok(true));
    }
}
