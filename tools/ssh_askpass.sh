#!/bin/sh
# Prints ROUTER_PASS for SSH_ASKPASS. Loads from env or repo .local/router.env.
set -e
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
ENV_FILE="${SOCKROCKET_LOCAL_DIR:-$ROOT/.local}/router.env"
if [ -z "${ROUTER_PASS:-}" ] && [ -f "$ENV_FILE" ]; then
  # shellcheck disable=SC1090
  . "$ENV_FILE"
fi
if [ -z "${ROUTER_PASS:-}" ]; then
  echo "ROUTER_PASS not set; copy .local.example/router.env to .local/router.env" >&2
  exit 1
fi
printf '%s\n' "$ROUTER_PASS"
