//! VLESS Encryption client adapted from meow-proxy 0.29.0 under MIT (see LICENSE).
//! Uses the same ring AEAD backend as the native TLS build.
mod aead;
mod client;
mod factory;
mod tasked_duplex;
pub(crate) use client::ClientInstance;
pub(crate) use factory::parse_client_encryption;
