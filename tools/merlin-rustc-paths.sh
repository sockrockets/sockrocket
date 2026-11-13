#!/bin/sh
# Keep the package and source file in panic messages without embedding the
# builder's home directory or Cargo registry hash. Cargo invokes this wrapper
# from each crate's directory, both locally and inside the cross container.
case "${CARGO_MANIFEST_DIR:-}" in
    */registry/src/*/*) exec "$@" "--remap-path-prefix=${CARGO_MANIFEST_DIR%/*}=/c" ;;
    *) exec "$@" ;;
esac
