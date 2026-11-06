use craft_tls::rustls::{
    BROTLI_DEFAULT, ZLIB_DEFAULT, ZSTD_DEFAULT, client_certificate_decompressor,
};

#[test]
fn client_decoders_preserve_full_codec_results_and_error_behavior() {
    for full in [BROTLI_DEFAULT, ZLIB_DEFAULT, ZSTD_DEFAULT] {
        let client = client_certificate_decompressor(full.alg).unwrap();
        assert_eq!(client.alg, full.alg);
        for len in [0, 1, 4096, 65536] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let encoded = full.provider.compress(Vec::new(), &payload).unwrap();
            let expected = full.provider.decompress(Vec::new(), &encoded).unwrap();
            assert_eq!(expected, payload);
            assert_eq!(
                client.provider.decompress(Vec::new(), &encoded).unwrap(),
                expected
            );
            let truncated = &encoded[..encoded.len() / 2];
            assert_eq!(
                client
                    .provider
                    .decompress(Vec::new(), truncated)
                    .map_err(|e| e.kind()),
                full.provider
                    .decompress(Vec::new(), truncated)
                    .map_err(|e| e.kind())
            );
        }
        assert_eq!(
            client
                .provider
                .compress(Vec::new(), b"unused")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
    }
}

#[test]
fn chrome_advertises_the_same_certificate_codec() {
    use craft_tls::rustls::{ClientConfig, RootCertStore, craft::CHROME_108};
    let config = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth()
        .with_fingerprint(CHROME_108.builder());
    assert_eq!(config.certificate_compression_algorithms.len(), 1);
    assert_eq!(
        config.certificate_compression_algorithms[0].alg,
        BROTLI_DEFAULT.alg
    );
}
