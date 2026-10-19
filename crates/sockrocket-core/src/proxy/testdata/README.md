These self-signed localhost TLS fixtures are public test data, not production credentials.
The private key must never be used by a deployed service. Tests trust only this certificate
and bind their server to a random loopback port. Generated using /CN=localhost with the
DNS:localhost SAN, serverAuth EKU and CA:FALSE.
