#!/bin/bash
# Full login_v2.cgi with all form fields, then check Set-Cookie.
# Credentials come from env or .local/router.env (never hardcoded).
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
ENV_FILE="${SOCKROCKET_LOCAL_DIR:-$ROOT/.local}/router.env"
if [ -f "$ENV_FILE" ]; then
  # shellcheck disable=SC1090
  set -a
  # shellcheck disable=SC1090
  . "$ENV_FILE"
  set +a
fi

: "${ROUTER_HOST:?set ROUTER_HOST in .local/router.env}"
: "${ROUTER_PASS:?set ROUTER_PASS in .local/router.env}"
ROUTER_PORT="${ROUTER_PORT:-22}"
ROUTER_USER="${ROUTER_USER:-admin}"
ID="${LOGIN_ID:-abcdefghij}"
CN="${LOGIN_CNONCE:-cccc1234cccc1234cccc1234cccc12}"

SSH=(sshpass -p "$ROUTER_PASS" ssh
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
  -p "$ROUTER_PORT"
  "${ROUTER_USER}@${ROUTER_HOST}")

get_nonce() {
  for _ in 1 2 3 4 5; do
    N=$("${SSH[@]}" "curl -sk --max-time 8 -X POST https://127.0.0.1:8443/get_Nonce.cgi -H 'Content-Type: application/json' -d '{\"id\":\"$ID\"}'" 2>/dev/null | sed 's/.*"nonce":"//;s/".*//')
    [ -n "$N" ] && { echo "$N"; return 0; }
    sleep 2
  done
  return 1
}

N=$(get_nonce) || { echo "nonce failed"; exit 1; }
echo "NONCE=$N"
H=$(printf '%s:%s:%s:%s' "$ROUTER_USER" "$N" "$ROUTER_PASS" "$CN" | sha256sum | cut -d' ' -f1)
echo "HASH=$H"

"${SSH[@]}" "curl -sk --max-time 8 -D /tmp/hdr.txt -X POST https://127.0.0.1:8443/login_v2.cgi \
 -d 'group_id=&action_mode=&action_script=&action_wait=5&current_page=Main_Login.asp&next_page=index.asp&login_authorization=$H&login_username=$ROUTER_USER&login_passwd=&login_captcha=&cnonce=$CN&id=$ID' \
 -o /tmp/lr.txt; echo BODY:; head -c 200 /tmp/lr.txt; echo; echo HEADERS:; cat /tmp/hdr.txt | grep -iE 'set-cookie|http/'"
