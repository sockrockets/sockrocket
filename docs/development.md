# Development guide

## Layout

```
sockrocket/
├── Cargo.toml
├── crates/
│   ├── sockrocket-core/
│   ├── sockrocket-gui/      # bin: sockrocket
│   ├── sockrocket-cli/      # bin: sockrocket-cli
│   └── craftls/
├── docs/
├── merlin/
├── tests/integration/
└── config.yaml.example
```

## Docs map

| Topic | Document |
|-------|----------|
| Architecture | [architecture.md](./architecture.md) |
| Configuration | [configuration.md](./configuration.md) |
| Protocols | [protocols.md](./protocols.md) |
| Internals | [internals/](./internals/) |
| Merlin | [merlin.md](./merlin.md), [merlin-reference.md](./merlin-reference.md) |

## Contributing

1. Fork and open a PR against `main`
2. Keep commits focused; CI must pass
3. Do not commit secrets (use `.local/` for machine-specific env files)

Pre-built binaries and Merlin packages are published via [GitHub Releases](https://github.com/sockrockets/sockrocket/releases).
