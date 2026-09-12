//! Verification of the signed DH key carried in an SMP server handshake.

use openssl::{
    pkey::{Id, PKey, PKeyRef, Public},
    sign::Verifier,
    x509::X509,
};
use thiserror::Error;

use crate::transport::{ServerHelloError, UnverifiedServerAuth};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifiedDhKey {
    X25519(Vec<u8>),
    X448(Vec<u8>),
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("server identity check failed: {0}")]
    Identity(#[from] ServerHelloError),
    #[error("signed server key has invalid DER encoding")]
    InvalidDer,
    #[error("signed server key uses an unsupported algorithm")]
    UnsupportedAlgorithm,
    #[error("signed server key signature is invalid")]
    BadSignature,
    #[error("TLS peer certificate or public key is invalid: {0}")]
    InvalidCertificate(#[source] openssl::error::ErrorStack),
}

impl UnverifiedServerAuth<'_> {
    /// Verify identity binding and the signed DH key using a peer certificate
    /// chain that the TLS implementation has already validated.
    pub fn verify_signed_key(
        &self,
        tls_peer_certificates: &[&[u8]],
        expected_fingerprint: &[u8; 32],
    ) -> Result<VerifiedDhKey, AuthError> {
        self.check_identity_binding(tls_peer_certificates, expected_fingerprint)?;
        let leaf =
            X509::from_der(tls_peer_certificates[0]).map_err(AuthError::InvalidCertificate)?;
        let leaf_key = leaf.public_key().map_err(AuthError::InvalidCertificate)?;
        verify_signed_key_with_key(self.signed_key, &leaf_key)
    }
}

fn verify_signed_key_with_key(
    signed_der: &[u8],
    certificate_key: &PKeyRef<Public>,
) -> Result<VerifiedDhKey, AuthError> {
    let (outer, rest) = take_tlv(signed_der, 0x30)?;
    if !rest.is_empty() {
        return Err(AuthError::InvalidDer);
    }
    let (subject_key, rest) = take_tlv(outer, 0x30)?;
    let subject_der = &outer[..outer.len() - rest.len()];
    let (_, subject_tail) = take_tlv(subject_key, 0x30)?;
    if !subject_tail.is_empty() {
        return Err(AuthError::InvalidDer);
    }
    let subject_public_key_der = &subject_key[..subject_key.len() - subject_tail.len()];
    let (algorithm, rest) = take_tlv(rest, 0x30)?;
    let (signature_bits, rest) = take_tlv(rest, 0x03)?;
    if !rest.is_empty() || signature_bits.first() != Some(&0) {
        return Err(AuthError::InvalidDer);
    }

    let (oid, rest) = take_tlv(algorithm, 0x06)?;
    if !rest.is_empty() {
        return Err(AuthError::UnsupportedAlgorithm);
    }
    let expected_key_type = match oid {
        [0x2b, 0x65, 0x70] => Id::ED25519,
        [0x2b, 0x65, 0x71] => Id::ED448,
        _ => return Err(AuthError::UnsupportedAlgorithm),
    };
    if certificate_key.id() != expected_key_type {
        return Err(AuthError::UnsupportedAlgorithm);
    }

    // `take_tlv` returns only the contents. The signature covers the complete
    // encoded SubjectPublicKeyInfo SEQUENCE, including its tag and length.
    let mut verifier =
        Verifier::new_without_digest(certificate_key).map_err(AuthError::InvalidCertificate)?;
    let valid = verifier
        .verify_oneshot(&signature_bits[1..], subject_der)
        .map_err(AuthError::InvalidCertificate)?;
    if !valid {
        return Err(AuthError::BadSignature);
    }

    let public_key =
        PKey::public_key_from_der(subject_public_key_der).map_err(|_| AuthError::InvalidDer)?;
    let raw = public_key
        .raw_public_key()
        .map_err(|_| AuthError::InvalidDer)?;
    match public_key.id() {
        Id::X25519 => Ok(VerifiedDhKey::X25519(raw)),
        Id::X448 => Ok(VerifiedDhKey::X448(raw)),
        _ => Err(AuthError::UnsupportedAlgorithm),
    }
}

// A strict DER TLV reader. Rejects indefinite and non-minimal lengths.
fn take_tlv(input: &[u8], tag: u8) -> Result<(&[u8], &[u8]), AuthError> {
    if input.first() != Some(&tag) {
        return Err(AuthError::InvalidDer);
    }
    let length_byte = *input.get(1).ok_or(AuthError::InvalidDer)?;
    let (length, header) = if length_byte < 0x80 {
        (length_byte as usize, 2)
    } else {
        let count = (length_byte & 0x7f) as usize;
        if count == 0 || count > 4 || input.len() < 2 + count || input[2] == 0 {
            return Err(AuthError::InvalidDer);
        }
        let mut length = 0usize;
        for byte in &input[2..2 + count] {
            length = (length << 8) | *byte as usize;
        }
        if length < 128 {
            return Err(AuthError::InvalidDer);
        }
        (length, 2 + count)
    };
    let end = header.checked_add(length).ok_or(AuthError::InvalidDer)?;
    let value = input.get(header..end).ok_or(AuthError::InvalidDer)?;
    Ok((value, &input[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::sign::Signer;

    fn tlv(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut encoded = vec![tag];
        if payload.len() < 128 {
            encoded.push(payload.len() as u8);
        } else {
            encoded.extend_from_slice(&[0x81, payload.len() as u8]);
        }
        encoded.extend_from_slice(payload);
        encoded
    }

    #[test]
    fn verifies_signed_x25519_key_and_rejects_tampering() {
        let signer = PKey::generate_ed25519().unwrap();
        let verifier_key = PKey::public_key_from_der(&signer.public_key_to_der().unwrap()).unwrap();
        let dh_key = PKey::generate_x25519().unwrap();
        let subject = tlv(0x30, &dh_key.public_key_to_der().unwrap());
        let signature = Signer::new_without_digest(&signer)
            .unwrap()
            .sign_oneshot_to_vec(&subject)
            .unwrap();
        let mut bits = vec![0];
        bits.extend_from_slice(&signature);
        let mut contents = subject.clone();
        contents.extend_from_slice(&tlv(0x30, &[0x06, 0x03, 0x2b, 0x65, 0x70]));
        contents.extend_from_slice(&tlv(0x03, &bits));
        let signed = tlv(0x30, &contents);

        assert_eq!(
            verify_signed_key_with_key(&signed, &verifier_key).unwrap(),
            VerifiedDhKey::X25519(dh_key.raw_public_key().unwrap())
        );

        let mut tampered = signed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(
            verify_signed_key_with_key(&tampered, &verifier_key),
            Err(AuthError::BadSignature)
        ));
        let other_signer = PKey::generate_ed25519().unwrap();
        let other_key =
            PKey::public_key_from_der(&other_signer.public_key_to_der().unwrap()).unwrap();
        assert!(matches!(
            verify_signed_key_with_key(&signed, &other_key),
            Err(AuthError::BadSignature)
        ));
        assert!(matches!(
            verify_signed_key_with_key(&signed[..signed.len() - 1], &verifier_key),
            Err(AuthError::InvalidDer)
        ));
    }
}
