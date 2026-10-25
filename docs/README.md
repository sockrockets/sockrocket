# Sockrocket documentation

Cross-platform proxy client (Rust): Shadowsocks, VMess, VLess, TUIC, Trojan, Hysteria2, and more.

## Website guides (preferred for users)

Readable pages on the marketing site (not raw GitHub Markdown):

| Page | Description |
|------|-------------|
| [Docs hub](./site/docs/) | Index of user-facing guides |
| [Getting started](./site/docs/getting-started.html) | Install, import, connect |
| [Routing](./site/docs/routing.html) | Rule mode, exits, node groups, presets and device defaults |
| [Merlin install](./site/docs/merlin.html) | Package pick + Softcenter install |
| [Configuration](./site/docs/configuration.html) | Everyday settings overview |
| [Protocols](./site/docs/protocols.html) | Protocol matrix and limits |

## Repository Markdown (full reference)

| Document | Description |
|----------|-------------|
| [Getting started](./getting-started.md) | Same content as the site guide, Markdown form |
| [Routing](./routing.md) | Rule mode, node exits, node groups, presets and device defaults |
| [Configuration](./configuration.md) | Full config file format and options |
| [Protocols](./protocols.md) | Detailed protocol and transport notes |
| [Merlin plugin](./merlin.md) | Full model tables and troubleshooting |
| [Merlin technical reference](./merlin-reference.md) | Router deployment details |

## Developer guides

| Document | Description |
|----------|-------------|
| [Development](./development.md) | Layout and contributing |
| [Privacy](./privacy-protection.md) | Local data, commit identity, and what not to share |
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
