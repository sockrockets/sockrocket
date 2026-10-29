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
| Merlin local build | [merlin-local-build.md](./merlin-local-build.md): ARM64 build command, allocator checks and provenance |

## Local Merlin ARM64 builds

Use `bash tools/build-merlin-musl.sh`. Zig compiles C dependencies; Rust's
`rust-lld` performs the final link with bundled musl. The build verifies allocator
symbols before stripping and saves a matching `.symbols` file. Do not use the Zig
C wrapper as the final Rust linker or copy the historical
`link-self-contained=no` workaround. See the [build guide](./merlin-local-build.md)
for prerequisites, artifact verification and the allocator retention incident.

## Contributing

1. Fork and open a PR against `main`
2. Keep commits focused; CI must pass
3. Do not commit secrets (use `.local/` for machine-specific env files)
4. Follow [Privacy](./privacy-protection.md) for commit identity, tags, and what may be shared

Pre-built binaries and Merlin packages are published via [GitHub Releases](https://github.com/sockrockets/sockrocket/releases).
