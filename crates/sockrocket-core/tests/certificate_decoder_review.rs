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

#[test]
fn browser_and_custom_fingerprints_keep_their_certificate_algorithms() {
    use craft_tls::rustls::{
        CertificateCompressionAlgorithm as Algorithm, ClientConfig, RootCertStore,
        craft::{self, CraftExtension, ExtensionSpec, Fingerprint},
    };
    let base = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    for profile in [
        craft::chrome_108(),
        craft::chrome_112(),
        craft::chrome_133(),
        craft::safari_17_1(),
        craft::edge_85(),
        craft::firefox_105(),
        craft::ios_14(),
        craft::qq_11_1(),
        craft::chrome_auto(),
        craft::CHROME_115_QUIC.clone(),
    ] {
        let all = base.clone().with_fingerprint(profile.builder());
        let small = base.clone().with_browser_fingerprint(profile.builder());
        assert_eq!(
            all.certificate_compression_algorithms.len(),
            small.certificate_compression_algorithms.len()
        );
        for (a, b) in all
            .certificate_compression_algorithms
            .iter()
            .zip(&small.certificate_compression_algorithms)
        {
            assert!(std::ptr::eq(*a, *b));
        }
    }
    static CUSTOM_EXTENSIONS: &[ExtensionSpec] =
        &[ExtensionSpec::Craft(CraftExtension::CompressCert(&[
            Algorithm::Zstd,
            Algorithm::Zlib,
        ]))];
    let custom = Fingerprint {
        extensions: CUSTOM_EXTENSIONS,
        ..Default::default()
    };
    let config = base.with_fingerprint(custom.builder());
    let result: Vec<_> = config
        .certificate_compression_algorithms
        .iter()
        .map(|a| a.alg)
        .collect();
    assert_eq!(result, [Algorithm::Zstd, Algorithm::Zlib]);
}
