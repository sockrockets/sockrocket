//! shadow-tls v3 client protocol support (!craft! extension).
//!
//! The shadow-tls server authenticates the client during the TLS handshake
//! by checking an HMAC signature hidden in the ClientHello's
//! legacy_session_id (so no extra round-trips or distinguishable bytes are
//! needed). The session id is `random[0..28] || HMAC[0..4]` where the HMAC
//! is keyed with the shared password and covers the whole ClientHello with
//! the signature tail of the session id zeroed. See the reference
//! implementation: <https://github.com/ihciah/shadow-tls> (client.rs
//! `generate_session_id`).

use alloc::sync::Arc;

use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::error::Error;
use crate::msgs::message::Message;

/// Client configuration for the shadow-tls v3 session id signature.
#[derive(Debug, Clone)]
pub struct ShadowTlsConfig {
    /// The shared shadow-tls password (plugin-opts.password).
    pub password: Arc<str>,
}

/// Patch the encoded ClientHello in place to carry the signed session id.
///
/// Mirrors shadow-tls' `generate_session_id`:
/// `hmac = HMAC-SHA1(key = password, msg = CH[0..39] || session_id(with zero
/// signature tail) || CH[71..])`, then `session_id[28..32] = hmac[0..4]`.
pub(crate) fn patch_client_hello(
    ch: &mut Message,
    config: &ShadowTlsConfig,
) -> Result<(), Error> {
    let encoded = match &mut ch.payload {
        crate::msgs::message::MessagePayload::Handshake { encoded, .. } => &mut encoded.0,
        _ => return Err(Error::General("shadow-tls: unexpected message payload".into())),
    };

    // Layout: handshake header (4) + client_version (2) + random (32) +
    // session_id length byte (1) => session id content at offset 39.
    if encoded.len() < 75 || encoded[38] != 32 {
        return Err(Error::General(
            "shadow-tls: unexpected ClientHello session id layout".into(),
        ));
    }

    // Ensure a fresh random prefix and a zeroed signature tail so the HMAC
    // input is well-defined regardless of what rustls generated.
    let random_prefix: [u8; 28] = rand::random();
    encoded[39..43].copy_from_slice(&random_prefix[0..4]);
    encoded[43..67].copy_from_slice(&random_prefix[4..28]);
    encoded[67..71].fill(0);

    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(config.password.as_bytes())
        .map_err(|_| Error::General("shadow-tls: HMAC init failed".into()))?;
    mac.update(&encoded[0..39]);
    mac.update(&encoded[39..71]);
    mac.update(&encoded[71..]);
    let sig = mac.finalize().into_bytes();

    encoded[67..71].copy_from_slice(&sig[0..4]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_client_hello() -> Message {
        // Minimal encoded handshake buffer with the session id at 39..71.
        let mut encoded = vec![0u8; 128];
        encoded[0] = 0x01; // HandshakeType::ClientHello
        encoded[38] = 32; // session id length
        let payload = crate::msgs::message::MessagePayload::Handshake {
            parsed: crate::msgs::handshake::HandshakeMessagePayload {
                typ: crate::msgs::enums::HandshakeType::ClientHello,
                payload: crate::msgs::handshake::HandshakePayload::ClientHello(
                    crate::msgs::handshake::ClientHelloPayload {
                        client_version: crate::msgs::enums::ProtocolVersion::TLSv1_2,
                        random: crate::msgs::handshake::Random::from([7u8; 32]),
                        session_id: crate::msgs::handshake::SessionId::random(
                            &crate::crypto::ring::default_provider().secure_random,
                        )
                        .unwrap(),
                        cipher_suites: vec![],
                        compression_methods: vec![],
                        extensions: vec![],
                    },
                ),
            },
            encoded: crate::msgs::codec::Payload(encoded),
        };
        Message {
            version: crate::msgs::enums::ProtocolVersion::TLSv1_0,
            payload,
        }
    }

    #[test]
    fn session_id_signature_is_deterministic() {
        // Two independent patch runs over identical CH bytes must produce
        // the same signature given the same random prefix; force the prefix
        // by patching, capturing the signed CH, then re-verifying the HMAC.
        let config = ShadowTlsConfig {
            password: Arc::from("10086"),
        };
        let mut ch = fake_client_hello();
        patch_client_hello(&mut ch, &config).unwrap();

        let encoded = match &ch.payload {
            crate::msgs::message::MessagePayload::Handshake { encoded, .. } => &encoded.0,
            _ => panic!("unexpected payload"),
        };

        // Recompute the HMAC over the CH with the signature tail zeroed and
        // compare with the embedded signature.
        let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(b"10086").unwrap();
        mac.update(&encoded[0..39]);
        let mut zeroed = encoded[39..71].to_vec();
        zeroed[28..32].fill(0);
        mac.update(&zeroed);
        mac.update(&encoded[71..]);
        let sig = mac.finalize().into_bytes();
        assert_eq!(encoded[67..71], sig[0..4]);
    }
}
