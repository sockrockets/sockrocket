# Sockrocket documentation

Cross-platform proxy client (Rust): Shadowsocks, VMess, VLess, TUIC, Trojan, Hysteria2, and more.

## User guides

| Document | Description |
|----------|-------------|
| [Getting started](./getting-started.md) | Install, configure, and first run |
| [Configuration](./configuration.md) | Config file format and options |
| [Protocols](./protocols.md) | Proxy protocols and transports |
| [Merlin plugin](./merlin.md) | AsusWRT-Merlin transparent proxy |
| [Merlin technical reference](./merlin-reference.md) | Router deployment details |

## Developer guides

| Document | Description |
|----------|-------------|
| [Development](./development.md) | Layout and contributing |
| [Architecture](./architecture.md) | Modules and data flow |
| [Internals](./internals/) | Protocol depth notes for contributors |

## Static site

[docs/site/](./site/) — marketing site sources.

## Repository layout

```
sockrocket/
├── Cargo.toml
├── crates/
│   ├── sockrocket-core/    # Core library
│   ├── sockrocket-gui/     # GUI (bin: sockrocket)
│   ├── sockrocket-cli/     # CLI (bin: sockrocket-cli)
│   └── craftls/            # Custom rustls fork
├── docs/                   # This directory
├── merlin/                 # Merlin router plugin
├── tests/integration/      # Shell integration tests
├── tools/                  # Ops helper scripts
└── config.yaml.example
```
