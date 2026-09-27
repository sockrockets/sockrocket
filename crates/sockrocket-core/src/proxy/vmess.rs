use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{Aead, AeadInPlace, KeyInit as AesKeyInit};
use anyhow::{Context as _, Result};
use md5::{Digest as _, Md5};
use rand::Rng;
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::config::model::TransportConfig;

use super::connector::{BoxProxyStream, Outbound};
use super::pool::ConnPool;
use super::transport::TlsConnFactory;

/// VMess AEAD security types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VMessSecurity {
    Aes128Gcm,
    Chacha20Poly1305,
    Auto,
    None,
    Zero,
}

impl VMessSecurity {
    fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "aes-128-gcm" => Self::Aes128Gcm,
            "chacha20-poly1305" | "chacha20-ietf-poly1305" => Self::Chacha20Poly1305,
            "auto" => Self::Aes128Gcm,
            "none" => Self::None,
            "zero" => Self::Zero,
            _ => Self::Aes128Gcm,
        }
    }

    fn byte(&self) -> u8 {
        match self {
            Self::Aes128Gcm => 0x03,
            Self::Chacha20Poly1305 => 0x04,
            Self::Auto => 0x03,
            Self::None => 0x05,
            Self::Zero => 0x06,
        }
    }
}

/// VMess AEAD outbound connector.
pub struct VMessOutbound {
    server: String,
    port: u16,
    uuid: [u8; 16],
    security: VMessSecurity,
    pool: ConnPool,
}

impl VMessOutbound {
    pub fn new(
        server: &str,
        port: u16,
        uuid_str: &str,
        cipher: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        Self::build(server, port, uuid_str, cipher, transport, true)
    }

    /// Probe/latency variant: the connection pool never warms in the
    /// background, so a one-shot latency test doesn't pre-connect a full
    /// pool of TLS handshakes at the server.
    pub(crate) fn new_unwarmed(
        server: &str,
        port: u16,
        uuid_str: &str,
        cipher: &str,
        transport: Option<&TransportConfig>,
    ) -> Result<Self> {
        Self::build(server, port, uuid_str, cipher, transport, false)
    }

    fn build(
        server: &str,
        port: u16,
        uuid_str: &str,
        cipher: &str,
        transport: Option<&TransportConfig>,
        warm_pool: bool,
    ) -> Result<Self> {
        let parsed_uuid = uuid::Uuid::parse_str(uuid_str)?;

        let factory = Arc::new(TlsConnFactory::from_transport(
            server, port, transport, false,
        ));
        let pool = if warm_pool {
            ConnPool::new(factory)
        } else {
            ConnPool::lazy(factory)
        };

        Ok(Self {
            server: server.to_string(),
            port,
            uuid: *parsed_uuid.as_bytes(),
            security: VMessSecurity::from_str(cipher),
            pool,
        })
    }
}

impl Outbound for VMessOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let target_host = host.to_string();
        Box::pin(async move {
            let stream = self.pool.get().await.with_context(|| {
                format!(
                    "VMess: failed to connect to {}:{} (target: {}:{})",
                    self.server, self.port, target_host, port
                )
            })?;

            // Generate session keys (in a block so rng drops before await)
            let (req_body_key, req_body_iv, resp_auth, header) = {
                let mut rng = rand::rng();
                let req_body_key: [u8; 16] = rng.random();
                let req_body_iv: [u8; 16] = rng.random();
                let resp_auth: u8 = rng.random();

                let header = build_vmess_header(
                    &self.uuid,
                    &req_body_key,
                    &req_body_iv,
                    resp_auth,
                    self.security,
                    &target_host,
                    port,
                )?;

                (req_body_key, req_body_iv, resp_auth, header)
            };

            // Derive response keys
            let resp_body_key_full = sha256_bytes(&req_body_key);
            let resp_body_iv_full = sha256_bytes(&req_body_iv);

            // Wrap in VMess AEAD stream
            let vmess_stream = VMessStream::new(
                stream,
                header,
                req_body_key,
                req_body_iv,
                resp_body_key_full[..16]
                    .try_into()
                    .expect("sha256 output is 32 bytes, first 16 is always valid"),
                resp_body_iv_full[..16]
                    .try_into()
                    .expect("sha256 output is 32 bytes, first 16 is always valid"),
                resp_auth,
                self.security,
            )
            .await?;

            Ok(Box::new(vmess_stream) as BoxProxyStream)
        })
    }

    fn name(&self) -> &str {
        "vmess"
    }
}

/// Derive the VMess user command key from UUID using MD5.
fn vmess_cmd_key(uuid: &[u8; 16]) -> [u8; 16] {
    let magic = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";
    let mut hasher = Md5::new();
    hasher.update(uuid);
    hasher.update(magic);
    let result = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&result);
    key
}

/// Create authenticated length for AEAD header.
/// VMess AEAD auth ID, matching xray's `aead.CreateAuthID`:
/// `AES-128-ECB(key = KDF16(cmd_key, "AES Auth ID Encryption"),
/// timestamp(8 BE) || random(4) || crc32_ieee(first 12)(4 BE))`.
fn create_auth_id(cmd_key: &[u8; 16], timestamp: u64) -> [u8; 16] {
    use aes::cipher::{BlockEncrypt, KeyInit};

    let key = kdf(cmd_key, &[b"AES Auth ID Encryption"]);
    let cipher = aes::Aes128::new_from_slice(&key[..16])
        .expect("KDF output is 32 bytes, first 16 is always a valid AES-128 key");

    let mut block = [0u8; 16];
    block[..8].copy_from_slice(&timestamp.to_be_bytes());
    let rand4: [u8; 4] = rand::rng().random();
    block[8..12].copy_from_slice(&rand4);
    let crc = crc32fast::hash(&block[..12]);
    block[12..16].copy_from_slice(&crc.to_be_bytes());

    cipher.encrypt_block((&mut block).into());
    block
}

/// Go-compatible HMAC-SHA256 state machine (needed to replicate xray's KDF,
/// which drives HMAC through a custom lazy hash wrapper).
#[derive(Clone)]
struct GoHmacSha256 {
    inner: Sha256,
    outer: Sha256,
}

impl GoHmacSha256 {
    fn new(key: &[u8]) -> Self {
        let mut block = [0u8; 64];
        if key.len() > 64 {
            let h = Sha256::digest(key);
            block[..32].copy_from_slice(&h);
        } else {
            block[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        inner.update(block.map(|b| b ^ 0x36));
        let mut outer = Sha256::new();
        outer.update(block.map(|b| b ^ 0x5c));
        Self { inner, outer }
    }

    fn write(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Go's `hmac.Sum(nil)`: `outer.digest(inner.digest())`, non-mutating.
    fn sum(&self) -> [u8; 32] {
        let inner_digest = self.inner.clone().finalize();
        let mut outer = self.outer.clone();
        outer.update(inner_digest);
        outer.finalize().into()
    }
}

/// KDF level: `inner` receives the message stream (starting with
/// `path⊕ipad`); `outer` is a frozen clone of `inner` taken before that
/// write, finalized with `path⊕opad` followed by inner's digest. Mirrors
/// v2ray-rust's VmessKdf1/2/3 (xray's lazy-hash chained HMAC KDF).
enum KdfHasher {
    Base(Box<GoHmacSha256>),
    Level(Box<KdfLevel>),
}

struct KdfLevel {
    okey: [u8; 64],
    inner: KdfHasher,
    outer: KdfHasher,
}

impl Clone for KdfHasher {
    fn clone(&self) -> Self {
        match self {
            KdfHasher::Base(h) => KdfHasher::Base(h.clone()),
            KdfHasher::Level(l) => KdfHasher::Level(Box::new(KdfLevel {
                okey: l.okey,
                inner: l.inner.clone(),
                outer: l.outer.clone(),
            })),
        }
    }
}

impl KdfHasher {
    fn new_level(inner: KdfHasher, key: &[u8]) -> KdfHasher {
        let outer = inner.clone();
        let mut inner = inner;
        inner.update(&hmac_block_pad(key, 0x36));
        KdfHasher::Level(Box::new(KdfLevel {
            okey: hmac_block_pad(key, 0x5c),
            inner,
            outer,
        }))
    }

    fn update(&mut self, m: &[u8]) {
        match self {
            KdfHasher::Base(h) => h.write(m),
            KdfHasher::Level(l) => l.inner.update(m),
        }
    }

    fn finalize(self) -> [u8; 32] {
        match self {
            KdfHasher::Base(h) => h.sum(),
            KdfHasher::Level(l) => {
                let h1 = l.inner.finalize();
                let mut outer = l.outer;
                outer.update(&l.okey);
                outer.update(&h1);
                outer.finalize()
            }
        }
    }
}

fn hmac_block_pad(key: &[u8], pad: u8) -> [u8; 64] {
    let mut block = [pad; 64];
    if key.len() > 64 {
        let h = Sha256::digest(key);
        for (i, b) in block.iter_mut().enumerate().take(32) {
            *b = pad ^ h[i];
        }
    } else {
        for (i, k) in key.iter().enumerate() {
            block[i] = pad ^ k;
        }
    }
    block
}

/// KDF for VMess AEAD key derivation, ported from v2ray-rust's kdf.rs
/// (byte-compatible with xray's `proxy/vmess/aead/kdf.go`); verified against
/// its known-answer test vector in `test_kdf`.
fn kdf(key: &[u8], paths: &[&[u8]]) -> Vec<u8> {
    let mut h = KdfHasher::Base(Box::new(GoHmacSha256::new(b"VMess AEAD KDF")));
    for path in paths {
        h = KdfHasher::new_level(h, path);
    }
    h.update(key);
    h.finalize().to_vec()
}

/// Build the full VMess AEAD request header bytes.
fn build_vmess_header(
    uuid: &[u8; 16],
    body_key: &[u8; 16],
    body_iv: &[u8; 16],
    resp_auth: u8,
    security: VMessSecurity,
    host: &str,
    port: u16,
) -> Result<Vec<u8>> {
    let cmd_key = vmess_cmd_key(uuid);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default() // fallback to epoch 0 on impossible clock-before-1970 error
        .as_secs();
    let auth_id = create_auth_id(&cmd_key, timestamp);

    let mut rng = rand::rng();
    let nonce: [u8; 8] = rng.random();

    // Build the inner header (to be encrypted)
    let mut header = Vec::with_capacity(128);

    // Version
    header.push(1);
    // Body IV
    header.extend_from_slice(body_iv);
    // Body Key
    header.extend_from_slice(body_key);
    // Response Auth
    header.push(resp_auth);
    // Option: standard chunk stream (0x01) only. Do NOT set 0x04 (chunk
    // masking): that makes the server Shake128-mask chunk lengths, which
    // VMessChunkCipher does not implement — the server would misparse the
    // length and wait forever (observed as a silent hang).
    header.push(0x01);
    // Padding length (4 bits) + Security (4 bits)
    let padding_len: u8 = rng.random::<u8>() % 16;
    header.push((padding_len << 4) | security.byte());
    // Reserved
    header.push(0);
    // Command: TCP
    header.push(0x01);

    // Port (big-endian)
    header.extend_from_slice(&port.to_be_bytes());

    // Address
    if let Ok(ipv4) = host.parse::<std::net::Ipv4Addr>() {
        header.push(0x01); // IPv4
        header.extend_from_slice(&ipv4.octets());
    } else if let Ok(ipv6) = host.parse::<std::net::Ipv6Addr>() {
        header.push(0x03); // IPv6
        header.extend_from_slice(&ipv6.octets());
    } else {
        header.push(0x02); // Domain
        anyhow::ensure!(
            host.len() <= 255,
            "Domain name too long for VMess: {} bytes (max 255)",
            host.len()
        );
        header.push(host.len() as u8);
        header.extend_from_slice(host.as_bytes());
    }

    // Random padding
    if padding_len > 0 {
        let padding: Vec<u8> = (0..padding_len).map(|_| rng.random()).collect();
        header.extend_from_slice(&padding);
    }

    // FNV1a hash of header for integrity
    let check = fnv1a32(&header);
    header.extend_from_slice(&check.to_be_bytes());

    // AEAD encrypt the header

    // Step 1: Derive header length encryption key and nonce
    let header_length_key_material = kdf(
        &cmd_key,
        &[b"VMess Header AEAD Key_Length", &auth_id, &nonce],
    );
    let header_length_nonce_material = kdf(
        &cmd_key,
        &[b"VMess Header AEAD Nonce_Length", &auth_id, &nonce],
    );

    let header_length_key: [u8; 16] = header_length_key_material[..16]
        .try_into()
        .map_err(|_| anyhow::anyhow!("KDF output too short for header length key"))?;
    let header_length_nonce: [u8; 12] = header_length_nonce_material[..12]
        .try_into()
        .map_err(|_| anyhow::anyhow!("KDF output too short for header length nonce"))?;

    let header_len = header.len() as u16;
    let cipher = Aes128Gcm::new_from_slice(&header_length_key)?;
    let encrypted_length = cipher
        .encrypt(
            (&header_length_nonce).into(),
            aes_gcm::aead::Payload {
                msg: &header_len.to_be_bytes(),
                aad: &auth_id,
            },
        )
        .map_err(|e| anyhow::anyhow!("AEAD encrypt length failed: {}", e))?;

    // Step 2: Derive header payload encryption key and nonce
    let header_key_material = kdf(&cmd_key, &[b"VMess Header AEAD Key", &auth_id, &nonce]);
    let header_nonce_material = kdf(&cmd_key, &[b"VMess Header AEAD Nonce", &auth_id, &nonce]);

    let header_key: [u8; 16] = header_key_material[..16]
        .try_into()
        .map_err(|_| anyhow::anyhow!("KDF output too short for header key"))?;
    let header_nonce: [u8; 12] = header_nonce_material[..12]
        .try_into()
        .map_err(|_| anyhow::anyhow!("KDF output too short for header nonce"))?;

    let cipher = Aes128Gcm::new_from_slice(&header_key)?;
    let encrypted_header = cipher
        .encrypt(
            (&header_nonce).into(),
            aes_gcm::aead::Payload {
                msg: &header,
                aad: &auth_id,
            },
        )
        .map_err(|e| anyhow::anyhow!("AEAD encrypt header failed: {}", e))?;

    // Assemble: auth_id(16) + encrypted_length(2+16) + nonce(8) + encrypted_header(N+16)
    let mut out = Vec::with_capacity(16 + 18 + 8 + encrypted_header.len());
    out.extend_from_slice(&auth_id);
    out.extend_from_slice(&encrypted_length);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&encrypted_header);

    Ok(out)
}

fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for &b in data {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// VMess AEAD encrypted stream.
///
/// A direct poll-based chunk codec over the inner stream (same shape as
/// `WsStream` / `ShadowTlsIo`) — no pump tasks, no duplex bridge, so payload
/// bytes cross the codec with a single copy per direction instead of two
/// copies plus a cross-task wakeup per chunk.
///
/// Read side state machine ([`ReadState`]): the response header is verified
/// lazily on the first read (xray servers hold it in a buffered writer until
/// request body bytes flow, so verifying eagerly in `new` would deadlock) —
/// assemble the 18-byte length block → assemble the payload block → verify —
/// then per chunk: 2-byte length prefix → ciphertext+tag assembled through
/// spare capacity → `decrypt_in_place` → plaintext delivered through
/// `plain_pos`, retaining leftovers larger than the caller's buffer across
/// polls. Write side: each `poll_write` takes up to `max_chunk` caller
/// bytes, encrypts in place behind the 2-byte length prefix in `out`, and
/// drains with saved position state, reporting the plaintext count only once
/// the chunk is fully written (`write_in_flight`).
struct VMessStream {
    inner: BoxProxyStream,

    // -- read side --
    read_cipher: VMessChunkCipher,
    state: ReadState,
    /// Response-header assembly buffer: the 18-byte length block, then the
    /// payload block. Allocation reused between the two blocks.
    resp_buf: Vec<u8>,
    /// Payload-block length revealed by the length block.
    resp_payload_len: usize,
    resp_key: [u8; 16],
    resp_iv: [u8; 16],
    resp_auth: u8,
    /// 2-byte chunk length prefix assembly.
    len_buf: [u8; 2],
    len_pos: usize,
    /// Length (ciphertext + tag) of the chunk being assembled.
    chunk_len: usize,
    /// Chunk buffer: ciphertext+tag during assembly, plaintext (truncated by
    /// `decrypt_in_place`) during delivery. Allocation reused across chunks.
    chunk: Vec<u8>,
    /// Delivery offset into the decrypted plaintext in `chunk`.
    plain_pos: usize,
    /// AEAD nonce counter, response direction (starts at 0 for the first
    /// body chunk after the response header, exactly as the old read task).
    read_count: u16,
    /// Response-header verification failure, surfaced once as a read error
    /// in place of a silent EOF (then reads are EOF).
    fatal: Option<io::Error>,
    /// Reads are done (zero-length chunk, inner EOF, or a mid-chunk
    /// failure — all silent EOFs, as the old read task defined them).
    read_eof: bool,

    // -- write side --
    write_cipher: VMessChunkCipher,
    /// Max caller bytes per chunk (the old write task's read-chunk size).
    max_chunk: usize,
    /// Wire bytes of the chunk being written: 2-byte length prefix +
    /// ciphertext + tag. Allocation reused across chunks.
    out: Vec<u8>,
    out_pos: usize,
    /// Plaintext bytes represented by the chunk currently draining in `out`,
    /// so a `poll_write` that pended mid-chunk knows what to report when it
    /// resumes.
    write_in_flight: Option<usize>,
    /// AEAD nonce counter, request direction.
    write_count: u16,
    /// The inner stream was shut down; further writes fail.
    write_closed: bool,
}

/// Read-side codec phase. Writes have no phase enum: a chunk in `out`
/// (`out_pos < out.len()`) is drained before any new bytes are taken.
enum ReadState {
    /// Assembling the 18-byte response-header length block into `resp_buf`.
    RespHeaderLen,
    /// Assembling the response-header payload block into `resp_buf`.
    RespHeaderPayload,
    /// Assembling the 2-byte chunk length prefix into `len_buf`.
    ChunkLen,
    /// Assembling a chunk's ciphertext+tag into `chunk`.
    ChunkPayload,
    /// Delivering decrypted plaintext from `chunk` (`plain_pos`).
    Plain,
}

enum VMessChunkCipher {
    Aes128Gcm {
        cipher: Box<Aes128Gcm>,
        iv: [u8; 16],
    },
    Chacha20Poly1305 {
        cipher: chacha20poly1305::ChaCha20Poly1305,
        iv: [u8; 16],
    },
    None,
}

impl VMessChunkCipher {
    fn new(security: VMessSecurity, key: &[u8; 16], iv: &[u8; 16]) -> Self {
        match security {
            VMessSecurity::Aes128Gcm | VMessSecurity::Auto => {
                let cipher =
                    Aes128Gcm::new_from_slice(key).expect("AES-128-GCM key is exactly 16 bytes");
                VMessChunkCipher::Aes128Gcm {
                    cipher: Box::new(cipher),
                    iv: *iv,
                }
            }
            VMessSecurity::Chacha20Poly1305 => {
                use chacha20poly1305::KeyInit;
                let key32 = generate_chacha_key(key);
                let cipher = chacha20poly1305::ChaCha20Poly1305::new_from_slice(&key32)
                    .expect("ChaCha20 key is exactly 32 bytes");
                VMessChunkCipher::Chacha20Poly1305 { cipher, iv: *iv }
            }
            VMessSecurity::None | VMessSecurity::Zero => VMessChunkCipher::None,
        }
    }

    /// Decrypt `data` in place, appending the 16-byte AEAD tag check in place.
    ///
    /// The previous implementation called `cipher.decrypt(...)` which returns a
    /// freshly allocated `Vec<u8>` for *every chunk* — a 1 MB download is ~64
    /// chunks, so that was 64 heap allocations plus a full copy of the payload
    /// per chunk. Using `decrypt_in_place` reuses the caller's buffer: zero
    /// allocations and zero extra copies on the read hot path. `data` must
    /// already contain the ciphertext+tag; on success it is truncated to the
    /// plaintext length.
    fn decrypt_in_place(&self, count: u16, data: &mut Vec<u8>) -> Result<()> {
        match self {
            VMessChunkCipher::Aes128Gcm { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .decrypt_in_place((&nonce).into(), b"", data)
                    .map_err(|e| anyhow::anyhow!("AES-GCM decrypt: {}", e))
            }
            VMessChunkCipher::Chacha20Poly1305 { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .decrypt_in_place((&nonce).into(), b"", data)
                    .map_err(|e| anyhow::anyhow!("ChaCha20 decrypt: {}", e))
            }
            VMessChunkCipher::None => Ok(()),
        }
    }

    /// Encrypt `data` in place, appending the AEAD tag onto the same buffer.
    /// The buffer must have `overhead()` bytes of spare capacity. Avoids the
    /// per-chunk `Vec` allocation + copy of the old `encrypt()`.
    ///
    /// Test-only: the production write path is [`Self::encrypt_payload_in_place`],
    /// which keeps the chunk length prefix in the same buffer.
    #[cfg(test)]
    fn encrypt_in_place(&self, count: u16, data: &mut Vec<u8>) -> Result<()> {
        match self {
            VMessChunkCipher::Aes128Gcm { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .encrypt_in_place((&nonce).into(), b"", data)
                    .map_err(|e| anyhow::anyhow!("AES-GCM encrypt: {}", e))
            }
            VMessChunkCipher::Chacha20Poly1305 { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                cipher
                    .encrypt_in_place((&nonce).into(), b"", data)
                    .map_err(|e| anyhow::anyhow!("ChaCha20 encrypt: {}", e))
            }
            VMessChunkCipher::None => Ok(()),
        }
    }

    /// Encrypt the payload at `data[start..]` in place, appending the
    /// detached AEAD tag to the buffer; `data[..start]` (the 2-byte chunk
    /// length prefix) is left untouched. Byte-identical on the wire to
    /// `encrypt_in_place` on the payload alone — used by the write codec,
    /// which keeps the length prefix and ciphertext in one buffer so a
    /// chunk drains with a single position counter.
    fn encrypt_payload_in_place(&self, count: u16, data: &mut Vec<u8>, start: usize) -> Result<()> {
        match self {
            VMessChunkCipher::Aes128Gcm { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                let tag = cipher
                    .encrypt_in_place_detached((&nonce).into(), b"", &mut data[start..])
                    .map_err(|e| anyhow::anyhow!("AES-GCM encrypt: {}", e))?;
                data.extend_from_slice(tag.as_slice());
                Ok(())
            }
            VMessChunkCipher::Chacha20Poly1305 { cipher, iv } => {
                let nonce = make_aead_nonce(iv, count);
                let tag = cipher
                    .encrypt_in_place_detached((&nonce).into(), b"", &mut data[start..])
                    .map_err(|e| anyhow::anyhow!("ChaCha20 encrypt: {}", e))?;
                data.extend_from_slice(tag.as_slice());
                Ok(())
            }
            VMessChunkCipher::None => Ok(()),
        }
    }

    fn overhead(&self) -> usize {
        match self {
            VMessChunkCipher::Aes128Gcm { .. } => 16,
            VMessChunkCipher::Chacha20Poly1305 { .. } => 16,
            VMessChunkCipher::None => 0,
        }
    }
}

// ChunkCipher is Send because both aes_gcm and chacha20poly1305 types are Send
unsafe impl Send for VMessChunkCipher {}

fn make_aead_nonce(iv: &[u8; 16], count: u16) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..2].copy_from_slice(&count.to_be_bytes());
    nonce[2..12].copy_from_slice(&iv[2..12]);
    nonce
}

fn generate_chacha_key(key: &[u8; 16]) -> [u8; 32] {
    let mut hasher1 = Md5::new();
    hasher1.update(key);
    let h1 = hasher1.finalize();

    let mut hasher2 = Md5::new();
    hasher2.update(h1);
    let h2 = hasher2.finalize();

    let mut key32 = [0u8; 32];
    key32[..16].copy_from_slice(&h1);
    key32[16..].copy_from_slice(&h2);
    key32
}

impl VMessStream {
    #[allow(clippy::too_many_arguments)]
    async fn new(
        mut inner: BoxProxyStream,
        header: Vec<u8>,
        req_key: [u8; 16],
        req_iv: [u8; 16],
        resp_key: [u8; 16],
        resp_iv: [u8; 16],
        resp_auth: u8,
        security: VMessSecurity,
    ) -> Result<Self> {
        inner.write_all(&header).await?;

        // The response header is verified lazily on the first `poll_read`,
        // NOT here: xray servers hold the tiny response header in a buffered
        // writer until request body bytes flow, so blocking connect() on the
        // response header deadlocks against real servers. A verification
        // failure surfaces as an error on the first read (see `fatal`).
        let write_cipher = VMessChunkCipher::new(security, &req_key, &req_iv);
        let max_chunk = 16384 - write_cipher.overhead();
        Ok(Self {
            inner,
            read_cipher: VMessChunkCipher::new(security, &resp_key, &resp_iv),
            state: ReadState::RespHeaderLen,
            resp_buf: Vec::with_capacity(18),
            resp_payload_len: 0,
            resp_key,
            resp_iv,
            resp_auth,
            len_buf: [0; 2],
            len_pos: 0,
            chunk_len: 0,
            chunk: Vec::with_capacity(16384 + 16),
            plain_pos: 0,
            read_count: 0,
            fatal: None,
            read_eof: false,
            write_cipher,
            max_chunk,
            out: Vec::with_capacity(2 + 16384),
            out_pos: 0,
            write_in_flight: None,
            write_count: 0,
            write_closed: false,
        })
    }

    /// Drain `out` (one chunk: length prefix + ciphertext + tag) into the
    /// inner stream, then flush it. The per-chunk flush matches the old
    /// write task: nothing else flushes the underlying (possibly
    /// buffered/TLS) stream.
    fn poll_flush_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            match Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    self.out.clear();
                    self.out_pos = 0;
                    return Poll::Ready(Err(e));
                }
                Poll::Ready(Ok(0)) => {
                    self.out.clear();
                    self.out_pos = 0;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "failed to write VMess chunk",
                    )));
                }
                Poll::Ready(Ok(n)) => self.out_pos += n,
            }
        }
        self.out.clear();
        self.out_pos = 0;
        Pin::new(&mut self.inner).poll_flush(cx)
    }
}

/// Fill `buf` from the inner stream until it holds `want` bytes, reading
/// through uninitialized spare capacity (zero-filling first would be a
/// wasted memset). EOF before `want` bytes is an UnexpectedEof error.
fn poll_fill_inner(
    inner: &mut BoxProxyStream,
    cx: &mut Context<'_>,
    buf: &mut Vec<u8>,
    want: usize,
) -> Poll<io::Result<()>> {
    buf.reserve(want.saturating_sub(buf.len()));
    while buf.len() < want {
        let filled = buf.len();
        let chunk = &mut buf.spare_capacity_mut()[..want - filled];
        let mut rb = ReadBuf::uninit(chunk);
        match Pin::new(&mut *inner).poll_read(cx, &mut rb) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {
                let n = rb.filled().len();
                if n == 0 {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "connection closed mid-read",
                    )));
                }
                // SAFETY: the ReadBuf above just initialized these `n` bytes
                // of spare capacity.
                unsafe { buf.set_len(filled + n) };
            }
        }
    }
    Poll::Ready(Ok(()))
}

/// Decrypt the response-header length block (2-byte length + 16-byte AEAD
/// tag), keyed with "AEAD Resp Header Len Key". Matches xray's
/// `ServerSession.EncodeResponseHeader`; keys derive from
/// sha256(request body key/iv).
fn decrypt_resp_header_len(
    enc_len: &[u8],
    resp_key: &[u8; 16],
    resp_iv: &[u8; 16],
) -> Result<usize> {
    let len_key = kdf(resp_key, &[b"AEAD Resp Header Len Key"]);
    let len_iv = kdf(resp_iv, &[b"AEAD Resp Header Len IV"]);
    let len_cipher = Aes128Gcm::new_from_slice(&len_key[..16])
        .map_err(|e| anyhow::anyhow!("resp header len key: {}", e))?;
    let plain_len = len_cipher
        .decrypt((&len_iv[..12]).into(), enc_len)
        .map_err(|e| anyhow::anyhow!("VMess response header length decrypt failed: {}", e))?;
    let payload_len = u16::from_be_bytes([plain_len[0], plain_len[1]]) as usize;
    anyhow::ensure!(
        payload_len <= 256,
        "VMess response header too large: {}",
        payload_len
    );
    Ok(payload_len)
}

/// Verify the response-header payload block (`payload` + 16-byte AEAD tag):
/// the payload is `[resp_auth, option] + command(2)`, AEAD-sealed keyed with
/// "AEAD Resp Header Key".
fn verify_resp_header_payload(
    enc_payload: &[u8],
    expected_auth: u8,
    resp_key: &[u8; 16],
    resp_iv: &[u8; 16],
) -> Result<()> {
    let payload_key = kdf(resp_key, &[b"AEAD Resp Header Key"]);
    let payload_iv = kdf(resp_iv, &[b"AEAD Resp Header IV"]);
    let payload_cipher = Aes128Gcm::new_from_slice(&payload_key[..16])
        .map_err(|e| anyhow::anyhow!("resp header payload key: {}", e))?;
    let payload = payload_cipher
        .decrypt((&payload_iv[..12]).into(), enc_payload)
        .map_err(|e| anyhow::anyhow!("VMess response header decrypt failed: {}", e))?;

    let resp_auth = payload.first().copied().unwrap_or(0);
    anyhow::ensure!(
        resp_auth == expected_auth,
        "VMess response auth mismatch: expected {}, got {}",
        expected_auth,
        resp_auth
    );
    Ok(())
}

impl AsyncRead for VMessStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let this = &mut *self;
            if buf.remaining() == 0 || this.read_eof {
                return Poll::Ready(Ok(()));
            }
            // A response-header verification failure surfaces once as the
            // read error (in place of the silent EOF), then reads are EOF —
            // the old pump stored `fatal` and shut the caller's stream down.
            if let Some(e) = this.fatal.take() {
                this.read_eof = true;
                return Poll::Ready(Err(e));
            }
            match this.state {
                ReadState::Plain => {
                    // Deliver decrypted plaintext; a leftover larger than the
                    // caller's buffer stays for the next poll.
                    let n = buf.remaining().min(this.chunk.len() - this.plain_pos);
                    buf.put_slice(&this.chunk[this.plain_pos..this.plain_pos + n]);
                    this.plain_pos += n;
                    if this.plain_pos == this.chunk.len() {
                        this.chunk.clear();
                        this.plain_pos = 0;
                        this.state = ReadState::ChunkLen;
                    }
                    return Poll::Ready(Ok(()));
                }
                ReadState::RespHeaderLen => {
                    // 2-byte length + 16-byte AEAD tag.
                    match poll_fill_inner(&mut this.inner, cx, &mut this.resp_buf, 18) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => {
                            // Same surface as the old read task: a failed
                            // verification becomes `fatal`.
                            tracing::error!("VMess response header verification failed: {e:#}");
                            this.fatal =
                                Some(io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")));
                            continue;
                        }
                        Poll::Ready(Ok(())) => {}
                    }
                    match decrypt_resp_header_len(&this.resp_buf, &this.resp_key, &this.resp_iv) {
                        Ok(len) => {
                            this.resp_payload_len = len;
                            this.resp_buf.clear();
                            this.state = ReadState::RespHeaderPayload;
                        }
                        Err(e) => {
                            tracing::error!("VMess response header verification failed: {e:#}");
                            this.fatal =
                                Some(io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")));
                        }
                    }
                }
                ReadState::RespHeaderPayload => {
                    let want = this.resp_payload_len + 16; // payload + AEAD tag
                    match poll_fill_inner(&mut this.inner, cx, &mut this.resp_buf, want) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(e)) => {
                            tracing::error!("VMess response header verification failed: {e:#}");
                            this.fatal =
                                Some(io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")));
                            continue;
                        }
                        Poll::Ready(Ok(())) => {}
                    }
                    match verify_resp_header_payload(
                        &this.resp_buf,
                        this.resp_auth,
                        &this.resp_key,
                        &this.resp_iv,
                    ) {
                        Ok(()) => {
                            this.resp_buf.clear();
                            this.state = ReadState::ChunkLen;
                        }
                        Err(e) => {
                            tracing::error!("VMess response header verification failed: {e:#}");
                            this.fatal =
                                Some(io::Error::new(io::ErrorKind::InvalidData, format!("{e:#}")));
                        }
                    }
                }
                ReadState::ChunkLen => {
                    while this.len_pos < 2 {
                        let mut rb = ReadBuf::new(&mut this.len_buf[this.len_pos..]);
                        match Pin::new(&mut this.inner).poll_read(cx, &mut rb) {
                            Poll::Pending => return Poll::Pending,
                            // Old read task: ANY failure of the length read
                            // (transport error or partial-then-EOF) ended the
                            // stream as a silent EOF.
                            Poll::Ready(Err(_)) => {
                                this.read_eof = true;
                                return Poll::Ready(Ok(()));
                            }
                            Poll::Ready(Ok(())) => {
                                let n = rb.filled().len();
                                if n == 0 {
                                    this.read_eof = true;
                                    return Poll::Ready(Ok(()));
                                }
                                this.len_pos += n;
                            }
                        }
                    }
                    this.len_pos = 0;
                    let chunk_len = u16::from_be_bytes(this.len_buf) as usize;
                    if chunk_len == 0 {
                        // Zero-length chunk: clean EOF (the old read task
                        // broke its loop here).
                        this.read_eof = true;
                        return Poll::Ready(Ok(()));
                    }
                    this.chunk_len = chunk_len;
                    this.chunk.clear();
                    this.state = ReadState::ChunkPayload;
                }
                ReadState::ChunkPayload => {
                    match poll_fill_inner(&mut this.inner, cx, &mut this.chunk, this.chunk_len) {
                        Poll::Pending => return Poll::Pending,
                        // Old read task: EOF/error mid-chunk ended the
                        // stream as a silent EOF.
                        Poll::Ready(Err(_)) => {
                            this.read_eof = true;
                            return Poll::Ready(Ok(()));
                        }
                        Poll::Ready(Ok(())) => {}
                    }
                    // Decrypt in place; `chunk` is truncated to the plaintext.
                    match this
                        .read_cipher
                        .decrypt_in_place(this.read_count, &mut this.chunk)
                    {
                        Ok(()) => {
                            this.read_count = this.read_count.wrapping_add(1);
                            this.plain_pos = 0;
                            if this.chunk.is_empty() {
                                // A tag-only chunk carries no bytes;
                                // delivering nothing would look like EOF, so
                                // skip to the next chunk.
                                this.state = ReadState::ChunkLen;
                            } else {
                                this.state = ReadState::Plain;
                            }
                        }
                        Err(e) => {
                            // Old read task logged and ended the stream as a
                            // silent EOF.
                            tracing::error!("VMess decrypt error: {}", e);
                            this.read_eof = true;
                            return Poll::Ready(Ok(()));
                        }
                    }
                }
            }
        }
    }
}

impl AsyncWrite for VMessStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.write_closed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "VMess stream is closed",
            )));
        }
        // A previous chunk may still be draining; finish it before taking
        // new bytes.
        if self.write_in_flight.is_some() || self.out_pos < self.out.len() {
            match self.poll_flush_out(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    self.write_in_flight = None;
                    return Poll::Ready(Err(e));
                }
                Poll::Ready(Ok(())) => {}
            }
            if let Some(n) = self.write_in_flight.take() {
                // Chunk fully on the wire: advance the request-direction
                // nonce counter, as the old write task did after its writes.
                self.write_count = self.write_count.wrapping_add(1);
                return Poll::Ready(Ok(n));
            }
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // One poll_write = one AEAD chunk, capped at max_chunk.
        let this = &mut *self;
        let n = buf.len().min(this.max_chunk);
        this.out.extend_from_slice(&[0, 0]); // length prefix placeholder
        this.out.extend_from_slice(&buf[..n]);
        if let Err(e) =
            this.write_cipher
                .encrypt_payload_in_place(this.write_count, &mut this.out, 2)
        {
            this.out.clear();
            return Poll::Ready(Err(io::Error::other(format!("{e:#}"))));
        }
        let chunk_len = (this.out.len() - 2) as u16;
        this.out[..2].copy_from_slice(&chunk_len.to_be_bytes());
        this.write_in_flight = Some(n);
        match this.poll_flush_out(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => {
                self.write_in_flight = None;
                Poll::Ready(Err(e))
            }
            Poll::Ready(Ok(())) => {
                self.write_in_flight = None;
                self.write_count = self.write_count.wrapping_add(1);
                Poll::Ready(Ok(n))
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Deliberately does not touch `write_in_flight`: the pended
        // `poll_write` still owes its caller the completion.
        self.poll_flush_out(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // VMess sends nothing on close (the old write task simply ended);
        // drain any in-flight chunk, then shut the inner stream down.
        match self.poll_flush_out(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        match Pin::new(&mut self.inner).poll_shutdown(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {
                self.write_closed = true;
                Poll::Ready(Ok(()))
            }
        }
    }
}

impl Unpin for VMessStream {}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    #[test]
    fn test_vmess_cmd_key() {
        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let key = vmess_cmd_key(uuid.as_bytes());
        // Just verify it produces a deterministic 16-byte key
        assert_eq!(key.len(), 16);
        let key2 = vmess_cmd_key(uuid.as_bytes());
        assert_eq!(key, key2);
    }

    #[test]
    fn test_create_auth_id() {
        use aes::cipher::{BlockDecrypt, KeyInit};

        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let cmd_key = vmess_cmd_key(uuid.as_bytes());
        let auth_id = create_auth_id(&cmd_key, 1000000);
        assert_eq!(auth_id.len(), 16);
        // A random nonce makes each auth id unique (anti-replay), even for
        // identical inputs.
        assert_ne!(
            create_auth_id(&cmd_key, 1000000),
            create_auth_id(&cmd_key, 1000000)
        );

        // Round-trip: decrypt with the same key xray's server side derives,
        // then verify the timestamp and CRC32 like `AuthIDDecoder.Match`.
        let key = kdf(&cmd_key, &[b"AES Auth ID Encryption"]);
        let cipher = aes::Aes128::new_from_slice(&key[..16]).unwrap();
        let mut block = auth_id;
        cipher.decrypt_block((&mut block).into());
        assert_eq!(&block[..8], &1000000u64.to_be_bytes());
        assert_eq!(&block[12..16], crc32fast::hash(&block[..12]).to_be_bytes());
    }

    #[test]
    fn test_kdf() {
        let key = b"test key material";
        let result = kdf(key, &[b"path1", b"path2"]);
        assert_eq!(result.len(), 32); // SHA256 output
        // Deterministic
        let result2 = kdf(key, &[b"path1", b"path2"]);
        assert_eq!(result, result2);
    }

    /// Known-answer test from v2ray-rust's kdf.rs test suite (proven
    /// byte-compatible with xray's KDF).
    #[test]
    fn test_kdf_known_answer() {
        let id = b"1234567890123456";
        let value = kdf(
            id,
            &[
                b"VMess Header AEAD Key_Length",
                b"AEAD Resp Header Len IV",
                b"AEAD Resp Header Len Key",
            ],
        );
        let expected: [u8; 32] = [
            0x27, 0x45, 0x93, 0x4f, 0x3b, 0x98, 0x7d, 0x07, 0x7b, 0x40, 0x82, 0xec, 0x0f, 0x76,
            0x06, 0x0f, 0x33, 0xd7, 0xf4, 0xd8, 0x9d, 0xd1, 0x72, 0xf4, 0x34, 0xc2, 0x75, 0xbf,
            0x91, 0xb1, 0x36, 0x0b,
        ];
        assert_eq!(value, expected);
    }

    #[test]
    fn test_fnv1a32() {
        assert_eq!(fnv1a32(b""), 0x811c9dc5);
        assert_eq!(fnv1a32(b"hello"), 0x4f9f2cab);
    }

    #[test]
    fn test_build_header() {
        let uuid = uuid::Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let body_key = [1u8; 16];
        let body_iv = [2u8; 16];

        let header = build_vmess_header(
            uuid.as_bytes(),
            &body_key,
            &body_iv,
            0x42,
            VMessSecurity::Aes128Gcm,
            "example.com",
            443,
        )
        .unwrap();

        // Header should be: auth_id(16) + enc_length(18) + nonce(8) + enc_header(N+16)
        assert!(header.len() > 16 + 18 + 8);
    }

    #[test]
    fn test_chunk_cipher_roundtrip_aes() {
        let key = [0xAA; 16];
        let iv = [0xBB; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::Aes128Gcm, &key, &iv);

        let plaintext = b"Hello, VMess AEAD!";
        // Encrypt in place: buffer starts as plaintext, gains a 16-byte tag.
        let mut buf = plaintext.to_vec();
        cipher.encrypt_in_place(0, &mut buf).unwrap();
        assert_eq!(buf.len(), plaintext.len() + 16);
        assert_ne!(&buf[..plaintext.len()], plaintext);

        // Decrypt in place: truncated back to the original plaintext.
        cipher.decrypt_in_place(0, &mut buf).unwrap();
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn test_chunk_cipher_roundtrip_chacha() {
        let key = [0xCC; 16];
        let iv = [0xDD; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::Chacha20Poly1305, &key, &iv);

        let plaintext = b"Hello, ChaCha20!";
        let mut buf = plaintext.to_vec();
        cipher.encrypt_in_place(0, &mut buf).unwrap();
        assert_eq!(buf.len(), plaintext.len() + 16);
        cipher.decrypt_in_place(0, &mut buf).unwrap();
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn test_chunk_cipher_none() {
        let key = [0; 16];
        let iv = [0; 16];
        let cipher = VMessChunkCipher::new(VMessSecurity::None, &key, &iv);

        let plaintext = b"No encryption";
        let mut buf = plaintext.to_vec();
        cipher.encrypt_in_place(0, &mut buf).unwrap();
        assert_eq!(buf, plaintext);
    }

    #[test]
    fn test_make_aead_nonce() {
        let iv = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10,
        ];
        let nonce = make_aead_nonce(&iv, 42);
        assert_eq!(nonce[0], 0); // 42 >> 8
        assert_eq!(nonce[1], 42); // 42 & 0xFF
        assert_eq!(&nonce[2..], &iv[2..12]);
    }

    #[test]
    fn test_vmess_outbound_new() {
        let outbound = VMessOutbound::new(
            "server.com",
            443,
            "b831381d-6324-4d53-ad4f-8cda48b30811",
            "aes-128-gcm",
            None,
        )
        .unwrap();
        assert_eq!(outbound.name(), "vmess");
    }

    #[test]
    fn test_security_from_str() {
        assert_eq!(
            VMessSecurity::from_str("aes-128-gcm"),
            VMessSecurity::Aes128Gcm
        );
        assert_eq!(
            VMessSecurity::from_str("chacha20-poly1305"),
            VMessSecurity::Chacha20Poly1305
        );
        // "auto" resolves to Aes128Gcm at parse time
        assert_eq!(VMessSecurity::from_str("auto"), VMessSecurity::Aes128Gcm);
        assert_eq!(VMessSecurity::from_str("none"), VMessSecurity::None);
        assert_eq!(VMessSecurity::from_str("zero"), VMessSecurity::Zero);
        // Auto resolves to AES-128-GCM byte
        assert_eq!(VMessSecurity::Auto.byte(), VMessSecurity::Aes128Gcm.byte());
    }

    /// Build a properly AEAD-sealed VMess response header the way xray's
    /// `EncodeResponseHeader` does it (len block + payload block). Takes the
    /// RESPONSE keys directly (sha256 of the request keys, first 16 bytes).
    fn seal_response_header(resp_auth: u8, resp_key: &[u8; 16], resp_iv: &[u8; 16]) -> Vec<u8> {
        // payload = [auth, option, cmd(2 zero bytes)]
        let payload = [resp_auth, 0x00, 0x00, 0x00];

        let len_key = kdf(resp_key, &[b"AEAD Resp Header Len Key"]);
        let len_iv = kdf(resp_iv, &[b"AEAD Resp Header Len IV"]);
        let len_cipher = Aes128Gcm::new_from_slice(&len_key[..16]).unwrap();
        let enc_len = len_cipher
            .encrypt(
                (&len_iv[..12]).into(),
                aes_gcm::aead::Payload {
                    msg: &(payload.len() as u16).to_be_bytes(),
                    aad: &[],
                },
            )
            .unwrap();

        let payload_key = kdf(resp_key, &[b"AEAD Resp Header Key"]);
        let payload_iv = kdf(resp_iv, &[b"AEAD Resp Header IV"]);
        let payload_cipher = Aes128Gcm::new_from_slice(&payload_key[..16]).unwrap();
        let enc_payload = payload_cipher
            .encrypt(
                (&payload_iv[..12]).into(),
                aes_gcm::aead::Payload {
                    msg: &payload,
                    aad: &[],
                },
            )
            .unwrap();

        let mut out = enc_len;
        out.extend_from_slice(&enc_payload);
        out
    }

    /// Response auth mismatch must surface as a READ error (not a silent EOF)
    /// so speedtests and relays see a real failure. Verification is lazy —
    /// eager verification deadlocks against xray's buffered response writer.
    #[tokio::test]
    async fn test_vmess_response_auth_mismatch_fails_connect() {
        let (client, mut server) = tokio::io::duplex(65536);
        let inner: BoxProxyStream = Box::new(client);
        let resp_auth = 0x42u8;

        let server_task = tokio::spawn(async move {
            // Consume the request header (single small write).
            let mut buf = [0u8; 64];
            let n = server.read(&mut buf).await.unwrap();
            eprintln!("[t] server read request header: {} bytes", n);
            // Respond with a sealed header carrying the WRONG auth byte.
            let resp = seal_response_header(resp_auth ^ 0xFF, &[3; 16], &[4; 16]);
            eprintln!("[t] server sealed resp: {} bytes", resp.len());
            server.write_all(&resp).await.unwrap();
            eprintln!("[t] server wrote resp");
        });

        eprintln!("[t] creating stream");
        let mut stream = VMessStream::new(
            inner,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            resp_auth,
            VMessSecurity::None,
        )
        .await
        .expect("stream creation itself must succeed (verify is lazy)");
        eprintln!("[t] stream created");

        let mut buf = [0u8; 16];
        let read_result =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("read hung >5s");
        eprintln!("[t] read returned: {:?}", read_result);
        let err = read_result.expect_err("auth mismatch must surface as a read error");
        assert!(
            format!("{err:#}").contains("auth mismatch"),
            "unexpected error: {err:#}"
        );
        server_task.await.unwrap();
    }

    /// A correct response auth byte passes lazy verification and the stream
    /// then delivers body bytes.
    #[tokio::test]
    async fn test_vmess_response_auth_match_connects() {
        let (client, mut server) = tokio::io::duplex(65536);
        let inner: BoxProxyStream = Box::new(client);
        let resp_auth = 0x42u8;

        let server_task = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let _ = server.read(&mut buf).await.unwrap();
            // Correct auth byte in a properly sealed header.
            let resp = seal_response_header(resp_auth, &[3; 16], &[4; 16]);
            server.write_all(&resp).await.unwrap();
            // One body chunk (security none): len(2 BE) + payload.
            server.write_all(&4u16.to_be_bytes()).await.unwrap();
            server.write_all(b"pong").await.unwrap();
        });

        let mut stream = VMessStream::new(
            inner,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            resp_auth,
            VMessSecurity::None,
        )
        .await
        .expect("stream creation must succeed");

        // Lazy verify consumed the sealed header; the body chunk follows.
        let mut got = [0u8; 4];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"pong");
        server_task.await.unwrap();
    }

    /// VMess over WebSocket: the request header and chunked body must ride
    /// inside WS frames, and framed response bytes must come back as a clean
    /// byte stream. The fake server hand-deframes WS to inspect the bytes.
    #[tokio::test]
    async fn test_vmess_over_ws_byte_level_roundtrip() {
        use base64::Engine as _;
        use sha1::Digest as _;

        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let resp_auth = 0x42u8;
        let resp_key = [3u8; 16];
        let resp_iv = [4u8; 16];

        let server_task = tokio::spawn(async move {
            // -- WS handshake: read request, answer 101.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 256];
            let head = loop {
                let n = server.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break String::from_utf8_lossy(&buf[..pos]).into_owned();
                }
            };
            assert!(head.starts_with("GET /ray HTTP/1.1"), "got: {head}");
            let key = head
                .split("\r\n")
                .find_map(|l| {
                    l.split_once(':').and_then(|(k, v)| {
                        k.trim()
                            .eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| v.trim().to_string())
                    })
                })
                .unwrap();
            let mut hasher = sha1::Sha1::new();
            hasher.update(key.as_bytes());
            hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
            server
                .write_all(
                    format!(
                        "HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();

            // -- Helper: read one masked client frame, return payload.
            async fn read_payload(server: &mut tokio::io::DuplexStream) -> Vec<u8> {
                let mut hdr = [0u8; 2];
                server.read_exact(&mut hdr).await.unwrap();
                assert!(hdr[1] & 0x80 != 0, "client frame must be masked");
                let mut len = (hdr[1] & 0x7f) as usize;
                if len == 126 {
                    let mut b = [0u8; 2];
                    server.read_exact(&mut b).await.unwrap();
                    len = u16::from_be_bytes(b) as usize;
                }
                let mut mask = [0u8; 4];
                server.read_exact(&mut mask).await.unwrap();
                let mut payload = vec![0u8; len];
                server.read_exact(&mut payload).await.unwrap();
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i % 4];
                }
                payload
            }

            // Frame 1: the VMess request header written by VMessStream::new.
            let header = read_payload(&mut server).await;
            assert_eq!(header, b"hdr", "VMess header must arrive unmodified");

            // Send the VMess response header in a WS frame (AEAD-sealed the
            // way a real server does).
            let resp_hdr = seal_response_header(resp_auth, &resp_key, &resp_iv);
            let mut frame = vec![0x82u8, resp_hdr.len() as u8];
            frame.extend_from_slice(&resp_hdr);
            server.write_all(&frame).await.unwrap();

            // Frame 2..N: one VMess chunk (security none): len(2 BE) +
            // payload. The chunk may span multiple WS frames (the WS layer
            // offers byte-stream semantics), so accumulate across frames.
            let mut acc = Vec::new();
            while acc.len() < 2 {
                acc.extend_from_slice(&read_payload(&mut server).await);
            }
            let payload_len = u16::from_be_bytes([acc[0], acc[1]]) as usize;
            while acc.len() < 2 + payload_len {
                acc.extend_from_slice(&read_payload(&mut server).await);
            }
            assert_eq!(&acc[2..2 + payload_len], b"ping");

            // Reply with a chunk frame carrying "pong".
            let mut reply = vec![0x82u8, 2 + 4];
            reply.extend_from_slice(&4u16.to_be_bytes());
            reply.extend_from_slice(b"pong");
            server.write_all(&reply).await.unwrap();
        });

        let ws_stream = crate::proxy::ws::connect_ws(
            Box::new(client) as BoxProxyStream,
            "server.example.com",
            &crate::config::model::WsConfig {
                path: Some("/ray".to_string()),
                host: None,
                headers: None,
            },
        )
        .await
        .expect("ws handshake must succeed");

        let mut stream = VMessStream::new(
            ws_stream,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            resp_auth,
            VMessSecurity::None,
        )
        .await
        .expect("vmess response header must verify");

        stream.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"pong");

        server_task.await.unwrap();
    }

    // -- Codec roundtrips (direct poll state machine) -----------------------

    /// Fake VMess server for codec tests: consumes the request header, sends
    /// a sealed response header, then echoes body chunks — decrypting with
    /// the request cipher and re-encrypting with the response cipher, each
    /// with its own nonce counter, exactly as a real server does.
    async fn fake_server_echo(
        mut server: tokio::io::DuplexStream,
        header_len: usize,
        resp_auth: u8,
        req_key: [u8; 16],
        req_iv: [u8; 16],
        resp_key: [u8; 16],
        resp_iv: [u8; 16],
        security: VMessSecurity,
    ) {
        let mut hdr = vec![0u8; header_len];
        server.read_exact(&mut hdr).await.unwrap();
        server
            .write_all(&seal_response_header(resp_auth, &resp_key, &resp_iv))
            .await
            .unwrap();
        let dec = VMessChunkCipher::new(security, &req_key, &req_iv);
        let enc = VMessChunkCipher::new(security, &resp_key, &resp_iv);
        let mut req_count = 0u16;
        let mut resp_count = 0u16;
        loop {
            let mut len_buf = [0u8; 2];
            if server.read_exact(&mut len_buf).await.is_err() {
                break;
            }
            let chunk_len = u16::from_be_bytes(len_buf) as usize;
            if chunk_len == 0 {
                break;
            }
            assert!(
                chunk_len <= 16384,
                "wire chunk exceeds the max chunk size: {chunk_len}"
            );
            let mut chunk = vec![0u8; chunk_len];
            if server.read_exact(&mut chunk).await.is_err() {
                break;
            }
            dec.decrypt_in_place(req_count, &mut chunk).unwrap();
            req_count = req_count.wrapping_add(1);
            enc.encrypt_in_place(resp_count, &mut chunk).unwrap();
            resp_count = resp_count.wrapping_add(1);
            server
                .write_all(&(chunk.len() as u16).to_be_bytes())
                .await
                .unwrap();
            server.write_all(&chunk).await.unwrap();
        }
    }

    /// AES-128-GCM encrypt→decrypt roundtrip of 200 KB through an in-memory
    /// duplex: multiple max-size chunks (16368 plaintext + 16 tag each) plus
    /// a tail, in both directions, with per-direction nonce sequencing.
    #[tokio::test]
    async fn test_vmess_stream_aes_multichunk_roundtrip() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(fake_server_echo(
            server,
            3,
            0x42,
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            VMessSecurity::Aes128Gcm,
        ));
        let stream = VMessStream::new(
            Box::new(client) as BoxProxyStream,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            0x42,
            VMessSecurity::Aes128Gcm,
        )
        .await
        .unwrap();

        // Split so writes and reads proceed concurrently: a full-duplex echo
        // of more data than the link buffer would deadlock a
        // write-all-then-read sequence (true for any buffered stream).
        let (mut rd, mut wr) = tokio::io::split(stream);
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let sent = data.clone();
        let write_task = tokio::spawn(async move { wr.write_all(&sent).await.unwrap() });
        let mut got = vec![0u8; data.len()];
        rd.read_exact(&mut got).await.unwrap();
        assert_eq!(got, data);
        write_task.await.unwrap();
        server_task.abort();
    }

    /// Response header and chunk bytes dribbling in one at a time must still
    /// decode through the poll state machine (partial length prefix, partial
    /// payload).
    #[tokio::test]
    async fn test_vmess_stream_read_tolerates_bytewise_delivery() {
        let (client, mut server) = tokio::io::duplex(8);
        let resp_auth = 0x42u8;
        let server_task = tokio::spawn(async move {
            let mut hdr = [0u8; 3];
            server.read_exact(&mut hdr).await.unwrap();
            let mut bytes = seal_response_header(resp_auth, &[3; 16], &[4; 16]);
            let mut chunk = b"trickle-vmess".to_vec();
            let enc = VMessChunkCipher::new(VMessSecurity::Aes128Gcm, &[3; 16], &[4; 16]);
            enc.encrypt_in_place(0, &mut chunk).unwrap();
            bytes.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            bytes.extend_from_slice(&chunk);
            for b in bytes {
                server.write_all(&[b]).await.unwrap();
            }
        });
        let mut stream = VMessStream::new(
            Box::new(client) as BoxProxyStream,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            resp_auth,
            VMessSecurity::Aes128Gcm,
        )
        .await
        .unwrap();
        let mut got = [0u8; 13];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"trickle-vmess");
        server_task.await.unwrap();
    }

    /// One chunk larger than the caller's buffer: the decrypted plaintext
    /// leftover must be retained across polls.
    #[tokio::test]
    async fn test_vmess_stream_chunk_delivered_across_small_reads() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let resp_auth = 0x42u8;
        let payload: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let server_task = tokio::spawn(async move {
            let mut hdr = [0u8; 3];
            server.read_exact(&mut hdr).await.unwrap();
            server
                .write_all(&seal_response_header(resp_auth, &[3; 16], &[4; 16]))
                .await
                .unwrap();
            let mut chunk = payload.clone();
            let enc = VMessChunkCipher::new(VMessSecurity::Chacha20Poly1305, &[3; 16], &[4; 16]);
            enc.encrypt_in_place(0, &mut chunk).unwrap();
            server
                .write_all(&(chunk.len() as u16).to_be_bytes())
                .await
                .unwrap();
            server.write_all(&chunk).await.unwrap();
        });
        let mut stream = VMessStream::new(
            Box::new(client) as BoxProxyStream,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            resp_auth,
            VMessSecurity::Chacha20Poly1305,
        )
        .await
        .unwrap();
        let mut got = vec![0u8; 5000];
        for part in got.chunks_mut(700) {
            stream.read_exact(part).await.unwrap();
        }
        let expected: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        assert_eq!(got, expected);
        server_task.await.unwrap();
    }

    /// A pended `poll_write` (chunk bigger than the link buffer) must resume
    /// mid-chunk and report the plaintext count only once fully written.
    #[tokio::test]
    async fn test_vmess_stream_pended_write_completes() {
        let (client, server) = tokio::io::duplex(1024);
        let server_task = tokio::spawn(fake_server_echo(
            server,
            3,
            0x42,
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            VMessSecurity::Aes128Gcm,
        ));
        let stream = VMessStream::new(
            Box::new(client) as BoxProxyStream,
            b"hdr".to_vec(),
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            0x42,
            VMessSecurity::Aes128Gcm,
        )
        .await
        .unwrap();

        // 40 KB = two max chunks + tail over a 1 KB link buffer: every chunk
        // write pends repeatedly while the server drains. Split so the echo
        // can be read while later chunks are still being written.
        let (mut rd, mut wr) = tokio::io::split(stream);
        let data: Vec<u8> = (0..40_000u32).map(|i| (i % 239) as u8).collect();
        let sent = data.clone();
        let write_task = tokio::spawn(async move { wr.write_all(&sent).await.unwrap() });
        let mut got = vec![0u8; data.len()];
        rd.read_exact(&mut got).await.unwrap();
        assert_eq!(got, data);
        write_task.await.unwrap();
        server_task.abort();
    }
}
