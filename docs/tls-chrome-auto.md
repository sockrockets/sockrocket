# Chrome Auto TLS profile

For ordinary TCP/TLS connections, `chrome`, `chromeauto`, `chrome133`, and the default fingerprint use the Chrome 133 profile selected by Mihomo v1.19.31 / MetaCubeX uTLS v1.8.8's `HelloChrome_Auto`. Auto is a build-time alias to a reviewed profile, not a runtime download or an assertion that every browser update is already implemented.

The profile advertises real X25519MLKEM768 and X25519 shares, keeps the private exchanges needed for either server choice, and retains P-256/P-384 support for HelloRetryRequest. It uses the new ALPS codepoint, randomized ECH GREASE, and shuffled extensions. GREASE does not encrypt SNI or enable real ECH. Existing certificate validation, leaf pins, SNI and explicit ALPN settings remain in effect.

`chrome112` and `chrome108` retain the classic profiles. REALITY retains its deliberate classic retry for old servers; QUIC's separate transport profile is not replaced by this TCP profile.

Tests inspect encoded ClientHello messages, including fresh shares, ECH GREASE stability across retry, and behavior after a cached classic key-exchange hint. Independent rustls servers verify actual hybrid and classic TLS 1.3 exchanges, a P-256 retry, and TLS 1.2 fallback. The independent AnyTLS server test covers TCP, UDP and session reuse.

References: [Mihomo fingerprint selection](https://github.com/MetaCubeX/mihomo/blob/v1.19.31/component/tls/utls.go), [uTLS Auto alias](https://github.com/MetaCubeX/utls/blob/v1.8.8/u_common.go), [Chrome 133 specification](https://github.com/MetaCubeX/utls/blob/v1.8.8/u_parrots.go).

Known validation limit: the optional independent Vision large-transfer test intermittently resets even on the unchanged baseline (4 of 10 fixed trials); both updated and explicit classic profiles also reproduced it. Passing ordinary TLS and AnyTLS checks does not establish that this separate Vision issue is fixed. Live network checks used a temporary, narrowly scoped router bypass to separate TLS client behavior from an additional proxy hop; the bypass was removed after testing.
