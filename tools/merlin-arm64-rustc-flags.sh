# Shared by the local and cross ARM64 builds.
# Optimize the CLI management code for size. Keep the core forwarding and
# protocol crates at the workspace optimization level.
merlin_arm64_cargo_flags=(
  --config 'profile.release.package.sockrocket-cli.opt-level="s"'
  --config 'profile.release.package.serde_yaml.opt-level="z"'
  --config 'profile.release.package.unsafe-libyaml.opt-level="z"'
  --config 'profile.release.package.regex-syntax.opt-level="z"'
  --config 'profile.release.package.regex-automata.opt-level="z"'
  # Use the same virtual source root for cross builds and key the dependency
  # cache separately from a build with unmodified source paths.
  --config 'target.aarch64-unknown-linux-musl.rustflags=["--remap-path-prefix=/project=/src"]'
  --config 'build.rustc-wrapper="tools/merlin-rustc-paths.sh"'
)

# Keep O3 on the forwarding path and fold address-insignificant sections.
merlin_arm64_flags=(
  -C llvm-args=--inline-threshold=80
  -C llvm-args=--addrsig
  -C link-arg=--icf=safe
  -C link-self-contained=yes
  -C strip=none
)
