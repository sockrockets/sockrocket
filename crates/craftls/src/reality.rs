//! REALITY client protocol support (!craft! extension).
//!
//! Implements the client side of the REALITY handshake camouflage protocol
//! (as used by Xray-core): the TLS 1.3 ClientHello's legacy_session_id is
//! repurposed to carry an AEAD-encrypted auth token derived from an X25519
//! ECDH between the client's ephemeral key share and the server's known
//! public key. The server proves its identity with a self-signed Ed25519
//! certificate whose signature field is HMAC-SHA512(auth_key, pubkey).

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::Aes256Gcm;
use hmac::{Hmac, Mac};
use ml_kem::EncodedSizeUser as _;
use pki_types::CertificateDer;
use rand::RngCore as _;
use sha2::Sha256;
use sha2::Sha512;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::crypto::{ActiveKeyExchange, SharedSecret, SupportedKxGroup};
use crate::error::Error;
use crate::msgs::handshake::Random;
use crate::msgs::message::Message;
use crate::NamedGroup;

/// Client configuration for the REALITY protocol.
#[derive(Debug, Clone)]
pub struct RealityConfig {
    /// The server's X25519 public key (raw 32 bytes).
    pub public_key: [u8; 32],
    /// The server's short id (up to 16 bytes; may be empty).
    pub short_id: Vec<u8>,
}

/// An X25519 key exchange group that retains the ephemeral private key so the
/// REALITY auth key can be derived from it. Drop-in replacement for the ring
/// X25519 group; use by putting it into `CryptoProvider::kx_groups` where the
/// X25519 group would normally be.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealityX25519KxGroup;

impl SupportedKxGroup for RealityX25519KxGroup {
    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }

    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let mut secret_bytes = [0u8; 32];
        ::rand::rngs::OsRng.fill_bytes(&mut secret_bytes);
        let secret = StaticSecret::from(secret_bytes);
        let public = PublicKey::from(&secret);
        Ok(Box::new(RealityX25519Kx {
            secret,
            public: public.to_bytes(),
        }))
    }
}

struct RealityX25519Kx {
    secret: StaticSecret,
    public: [u8; 32],
}

impl core::fmt::Debug for RealityX25519Kx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RealityX25519Kx")
            .field("public", &self.public)
            .finish()
    }
}

impl ActiveKeyExchange for RealityX25519Kx {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let peer: [u8; 32] = peer_pub_key.try_into().map_err(|_| {
            Error::General("REALITY: invalid peer X25519 public key length".into())
        })?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(peer));
        Ok(SharedSecret::from(&shared.to_bytes()[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }

    fn secret_key(&self) -> Option<&[u8]> {
        Some(self.secret.as_bytes())
    }
}

// --- X25519MLKEM768 hybrid (required by modern REALITY servers) ---

/// ML-KEM-768 encapsulation key size.
const MLKEM768_EK_SIZE: usize = 1184;
/// ML-KEM-768 ciphertext size.
const MLKEM768_CT_SIZE: usize = 1088;

/// The `X25519MLKEM768` hybrid key exchange group. The key share on the wire
/// is `ML-KEM-768 encapsulation key || X25519 public key`; the peer's share is
/// `ML-KEM-768 ciphertext || X25519 public key`; the TLS shared secret is
/// `ML-KEM-768 shared secret || X25519 shared secret`. The X25519 private key
/// is retained so the REALITY auth key can be derived from it.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealityX25519MlKem768Group;

impl SupportedKxGroup for RealityX25519MlKem768Group {
    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let mut rng = ::rand::rngs::OsRng;
        let (mlkem_dk, mlkem_ek) = {
            use ml_kem::KemCore as _;
            ml_kem::MlKem768::generate(&mut rng)
        };

        let mut x_secret_bytes = [0u8; 32];
        rng.fill_bytes(&mut x_secret_bytes);
        let x_secret = StaticSecret::from(x_secret_bytes);
        let x_public = PublicKey::from(&x_secret);

        let mut public = Vec::with_capacity(MLKEM768_EK_SIZE + 32);
        public.extend_from_slice(mlkem_ek.as_bytes().as_slice());
        public.extend_from_slice(&x_public.to_bytes());

        Ok(Box::new(RealityX25519MlKem768Kx {
            mlkem_dk,
            x_secret,
            x_public: x_public.to_bytes(),
            public,
        }))
    }
}

struct RealityX25519MlKem768Kx {
    mlkem_dk: ml_kem::kem::DecapsulationKey<ml_kem::MlKem768Params>,
    x_secret: StaticSecret,
    x_public: [u8; 32],
    public: Vec<u8>,
}

impl core::fmt::Debug for RealityX25519MlKem768Kx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RealityX25519MlKem768Kx")
            .field("x_public", &self.x_public)
            .finish()
    }
}

impl ActiveKeyExchange for RealityX25519MlKem768Kx {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        use ml_kem::kem::Decapsulate;

        if peer_pub_key.len() != MLKEM768_CT_SIZE + 32 {
            return Err(Error::General(format!(
                "REALITY: invalid X25519MLKEM768 peer key length {}",
                peer_pub_key.len()
            )));
        }
        let ct = &peer_pub_key[..MLKEM768_CT_SIZE];
        let x_peer: [u8; 32] = peer_pub_key[MLKEM768_CT_SIZE..]
            .try_into()
            .map_err(|_| Error::General("REALITY: invalid peer X25519 key".into()))?;

        let ct_array: ml_kem::Ciphertext<ml_kem::MlKem768> = ct
            .try_into()
            .map_err(|_| Error::General("REALITY: invalid ML-KEM ciphertext".into()))?;
        let mlkem_shared = self
            .mlkem_dk
            .decapsulate(&ct_array)
            .map_err(|_| Error::General("REALITY: ML-KEM decapsulation failed".into()))?;

        let x_shared = self.x_secret.diffie_hellman(&PublicKey::from(x_peer));

        let mut secret = Vec::with_capacity(64);
        secret.extend_from_slice(mlkem_shared.as_slice());
        secret.extend_from_slice(&x_shared.to_bytes());
        Ok(SharedSecret::from(&secret[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn secret_key(&self) -> Option<&[u8]> {
        Some(self.x_secret.as_bytes())
    }
}

/// Compute the REALITY auth key:
/// `HKDF-SHA256(salt = client_random[0..20], ikm = X25519(eph_priv, server_pk), info = "REALITY")`.
pub(crate) fn compute_auth_key(
    eph_secret: &[u8],
    server_public_key: &[u8; 32],
    client_random: &Random,
) -> Result<[u8; 32], Error> {
    let secret = StaticSecret::from(
        <[u8; 32]>::try_from(eph_secret)
            .map_err(|_| Error::General("REALITY: invalid ephemeral secret".into()))?,
    );
    let shared = secret.diffie_hellman(&PublicKey::from(*server_public_key));
    let hk = hkdf::Hkdf::<Sha256>::new(Some(&client_random.0[..20]), &shared.to_bytes());
    let mut auth_key = [0u8; 32];
    hk.expand(b"REALITY", &mut auth_key)
        .map_err(|_| Error::General("REALITY: HKDF expansion failed".into()))?;
    Ok(auth_key)
}

/// Patch the encoded ClientHello in place to carry the REALITY auth token,
/// and output the auth key for later certificate verification.
///
/// Mirrors Xray-core's `reality.UClient`:
/// session_id = [version(3) | reserved(1) | u32be unix time | short_id | zeros],
/// then `AES-256-GCM(auth_key).seal(nonce = client_random[20..32],
/// plaintext = session_id[..16], aad = encoded ClientHello with session id
/// field zeroed)`, the 32-byte ciphertext+tag replacing the session id.
pub(crate) fn patch_client_hello(
    ch: &mut Message,
    random: &Random,
    config: &RealityConfig,
    kx: Option<&dyn ActiveKeyExchange>,
    auth_key_out: &mut Option<Arc<[u8; 32]>>,
) -> Result<(), Error> {
    let encoded = match &mut ch.payload {
        crate::msgs::message::MessagePayload::Handshake { encoded, .. } => &mut encoded.0,
        _ => return Err(Error::General("REALITY: unexpected message payload".into())),
    };

    // Layout check: handshake header (4) + client_version (2) + random (32)
    // + session_id length byte (1) => session id content at offset 39.
    if encoded.len() < 71 || encoded[38] != 32 {
        return Err(Error::General(
            "REALITY: unexpected ClientHello session id layout".into(),
        ));
    }

    let kx = kx.ok_or_else(|| {
        Error::General("REALITY: no key exchange available for auth key".into())
    })?;
    let secret = kx.secret_key().ok_or_else(|| {
        Error::General(
            "REALITY: the X25519 key exchange does not expose its private key; \
             ensure the provider uses RealityX25519KxGroup"
                .into(),
        )
    })?;

    // Sanity: the key share we authenticate must be the one on the wire.
    let pub_key = kx.pub_key();
    let on_wire = encoded
        .windows(pub_key.len())
        .any(|window| window == pub_key);
    if !on_wire {
        return Err(Error::General(
            "REALITY: the negotiated key share is not present in the ClientHello"
                .into(),
        ));
    }

    if config.short_id.len() > 16 {
        return Err(Error::General(
            "REALITY: short id longer than 16 bytes".into(),
        ));
    }

    let mut session_id = [0u8; 32];
    // session_id[0..4]: Xray core version + reserved byte. Some servers gate
    // on MinClientVer/MaxClientVer — claim the version of the current Xray
    // release line (25.10.15), which sing-box/xray clients actually report.
    session_id[0] = 25;
    session_id[1] = 10;
    session_id[2] = 15;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::General("REALITY: system clock error".into()))?;
    session_id[4..8].copy_from_slice(&(now.as_secs() as u32).to_be_bytes());
    session_id[8..8 + config.short_id.len()].copy_from_slice(&config.short_id);

    let auth_key = compute_auth_key(secret, &config.public_key, random)?;

    // AAD: the full encoded ClientHello with the session id field zeroed.
    let mut aad = encoded.clone();
    aad[39..71].fill(0);

    let cipher = Aes256Gcm::new_from_slice(&auth_key)
        .map_err(|_| Error::General("REALITY: AES key init failed".into()))?;
    let nonce = aes_gcm::Nonce::from_slice(&random.0[20..32]);
    let sealed = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &session_id[..16],
                aad: &aad,
            },
        )
        .map_err(|_| Error::General("REALITY: session id encryption failed".into()))?;
    debug_assert_eq!(sealed.len(), 32);

    encoded[39..71].copy_from_slice(&sealed);
    *auth_key_out = Some(Arc::new(auth_key));
    Ok(())
}

// --- Certificate verification ---

/// A minimal DER TLV reader.
struct DerReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> DerReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Read one TLV; returns (tag, content).
    fn read_tlv(&mut self) -> Result<(u8, &'a [u8]), Error> {
        if self.pos + 2 > self.buf.len() {
            return Err(Error::General("REALITY: truncated DER".into()));
        }
        let tag = self.buf[self.pos];
        let mut len = self.buf[self.pos + 1] as usize;
        let mut hdr = 2;
        if len & 0x80 != 0 {
            let n = len & 0x7f;
            if n == 0 || n > 4 || self.pos + 2 + n > self.buf.len() {
                return Err(Error::General("REALITY: bad DER length".into()));
            }
            len = 0;
            for i in 0..n {
                len = (len << 8) | self.buf[self.pos + 2 + i] as usize;
            }
            hdr += n;
        }
        if self.pos + hdr + len > self.buf.len() {
            return Err(Error::General("REALITY: DER content out of bounds".into()));
        }
        let content = &self.buf[self.pos + hdr..self.pos + hdr + len];
        self.pos += hdr + len;
        Ok((tag, content))
    }
}

/// Extract (tbs_certificate, signature_value) from a DER X.509 certificate.
fn parse_certificate(der: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let mut outer = DerReader::new(der);
    let (tag, cert_body) = outer.read_tlv()?;
    if tag != 0x30 {
        return Err(Error::General("REALITY: certificate is not a SEQUENCE".into()));
    }
    let mut reader = DerReader::new(cert_body);
    let (tbs_tag, tbs) = reader.read_tlv()?;
    if tbs_tag != 0x30 {
        return Err(Error::General(
            "REALITY: tbsCertificate is not a SEQUENCE".into(),
        ));
    }
    let (alg_tag, _) = reader.read_tlv()?;
    if alg_tag != 0x30 {
        return Err(Error::General(
            "REALITY: signatureAlgorithm is not a SEQUENCE".into(),
        ));
    }
    let (sig_tag, sig_content) = reader.read_tlv()?;
    if sig_tag != 0x03 || sig_content.is_empty() {
        return Err(Error::General(
            "REALITY: signatureValue is not a BIT STRING".into(),
        ));
    }
    // First content byte is the unused-bits count; must be 0.
    if sig_content[0] != 0 {
        return Err(Error::General(
            "REALITY: unexpected signatureValue unused bits".into(),
        ));
    }
    Ok((tbs, &sig_content[1..]))
}

/// Extract the raw Ed25519 public key (32 bytes) from a
/// SubjectPublicKeyInfo DER, checking the Ed25519 OID (1.3.101.112).
fn parse_ed25519_spki(spki: &[u8]) -> Result<[u8; 32], Error> {
    // 30 2a | 30 05 06 03 2b 65 70 (alg id) | 03 21 00 <32-byte key>
    const ED25519_SPKI_PREFIX: &[u8] = &[0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70];
    let mut outer = DerReader::new(spki);
    let (tag, spki_body) = outer.read_tlv()?;
    if tag != 0x30 {
        return Err(Error::General("REALITY: SPKI is not a SEQUENCE".into()));
    }
    if !spki_body.starts_with(ED25519_SPKI_PREFIX) {
        return Err(Error::General(
            "REALITY: certificate is not an Ed25519 certificate".into(),
        ));
    }
    // BIT STRING content: 0x03 len 0x00 <32-byte key>, at the tail.
    if spki_body.len() < 35 || tail_check(spki_body).is_err() {
        return Err(Error::General(format!(
            "REALITY: malformed Ed25519 public key BIT STRING (spki_body {:02x?})",
            spki_body
        )));
    }
    Ok(tail_check(spki_body).unwrap())
}

fn tail_check(spki_body: &[u8]) -> Result<[u8; 32], Error> {
    let tail = &spki_body[spki_body.len() - 35..];
    // 0x03 = BIT STRING, 0x21 = 33 content bytes, 0x00 = unused bits.
    if tail[0] != 0x03 || tail[1] != 33 || tail[2] != 0 {
        return Err(Error::General("bad".into()));
    }
    Ok(<[u8; 32]>::try_from(&tail[3..35]).unwrap())
}

/// Verify the REALITY server certificate: it must be an Ed25519 certificate
/// whose signature field equals `HMAC-SHA512(auth_key, ed25519_pubkey)`.
pub(crate) fn verify_end_entity(end_entity: &CertificateDer<'_>, auth_key: &[u8; 32]) -> Result<(), Error> {
    let (tbs, signature) = parse_certificate(end_entity.as_ref())?;

    // The public key is inside tbsCertificate: skip serialNumber,
    // signature (AlgorithmIdentifier), issuer, validity, subject; the
    // subjectPublicKeyInfo follows. An Ed25519 SPKI is exactly:
    // 30 2a | 30 05 06 03 2b 65 70 | 03 21 00 <32-byte key>
    const SPKI_PREFIX: &[u8] = &[0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70];
    let spki_at = tbs
        .windows(SPKI_PREFIX.len() + 3)
        .position(|window| window.starts_with(SPKI_PREFIX))
        .ok_or_else(|| {
            Error::General(format!(
                "REALITY: Ed25519 SPKI not found in certificate \
                 (len {}, prefix {:02x?})",
                end_entity.len(),
                &end_entity.as_ref()[..end_entity.len().min(24)],
            ))
        })?;
    let spki = &tbs[spki_at..];
    let public_key = parse_ed25519_spki(spki)?;

    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(auth_key)
        .map_err(|_| Error::General("REALITY: HMAC init failed".into()))?;
    mac.update(&public_key);
    let expected = mac.finalize().into_bytes();
    if expected.as_slice() != signature {
        return Err(Error::General(
            "REALITY: server certificate authentication failed \
             (possible MITM or redirection to the real target)"
                .into(),
        ));
    }
    Ok(())
}

/// Verify the TLS 1.3 CertificateVerify signature made with the REALITY
/// server's Ed25519 key, using webpki's raw signature verification.
pub(crate) fn verify_cert_verify_signature(
    end_entity: &CertificateDer<'_>,
    message: &[u8],
    signature: &[u8],
) -> Result<(), Error> {
    let cert = webpki::EndEntityCert::try_from(end_entity)
        .map_err(|_| Error::General("REALITY: failed to parse server certificate".into()))?;
    cert.verify_signature(webpki_algs_ed25519(), message, signature)
        .map_err(|_| Error::General("REALITY: invalid CertificateVerify signature".into()))
}

/// The Ed25519 signature verification algorithm from webpki (ring-backed).
fn webpki_algs_ed25519() -> &'static dyn pki_types::SignatureVerificationAlgorithm {
    #[cfg(feature = "ring")]
    {
        webpki::ring::ED25519
    }
    #[cfg(all(feature = "aws_lc_rs", not(feature = "ring")))]
    {
        webpki::aws_lc_rs::ED25519
    }
}
