#!/usr/bin/env bash
# Safe Merlin deploy: upload first, then restart in a detached on-router job so
# SSH dropping when transparent proxy/DNS flaps does not leave a half-swapped
# binary. After restart, poll until SSH returns and verify the service.
#
# Usage (from repo root):
#   . .local/router.env   # ROUTER_HOST/PORT/USER/PASS
#   ./tools/merlin_deploy.sh
#
# Env overrides:
#   BIN=.../sockrocket-cli   ASP=.../sockrocket.asp   SH=.../sockrocket.sh
#   SKIP_RESTART=1           # upload only (no stop/start)
#   FORCE_RESTART=1          # restart even when remote md5 matches

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=/dev/null
[[ -f "$ROOT/.local/router.env" ]] && . "$ROOT/.local/router.env"

HOST="${ROUTER_HOST:?set ROUTER_HOST in .local/router.env}"
PORT="${ROUTER_PORT:-22}"
USER="${ROUTER_USER:?set ROUTER_USER in .local/router.env}"
PASS="${ROUTER_PASS:?set ROUTER_PASS in .local/router.env}"

BIN="${BIN:-$ROOT/target/aarch64-unknown-linux-musl/release/sockrocket-cli}"
ASP="${ASP:-$ROOT/merlin/webui/sockrocket.asp}"
SH="${SH:-$ROOT/merlin/scripts/sockrocket.sh}"
REMOTE_DIR="/jffs/addons/sockrocket"

[[ -x "$BIN" ]] || { echo "missing binary: $BIN"; exit 1; }
[[ -f "$ASP" ]] || { echo "missing asp: $ASP"; exit 1; }
[[ -f "$SH" ]] || { echo "missing script: $SH"; exit 1; }

SSH_OPTS=(-p "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
  -o PreferredAuthentications=password -o PubkeyAuthentication=no
  -o ConnectTimeout=15)
SCP_OPTS=(-O -P "$PORT" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
  -o PreferredAuthentications=password -o PubkeyAuthentication=no
  -o ConnectTimeout=30)

export DISPLAY="${DISPLAY:-:0}"
export SSH_ASKPASS="${SSH_ASKPASS:-$ROOT/tools/ssh_askpass.sh}"
export SSH_ASKPASS_REQUIRE=force
export ROUTER_PASS="$PASS"

ssh_do() { ssh "${SSH_OPTS[@]}" "$USER@$HOST" "$@"; }
scp_do() { scp "${SCP_OPTS[@]}" "$1" "$USER@$HOST:$2"; }

echo "==> 1/4 upload (network still up)"
LOCAL_MD5=$(md5 -q "$BIN" 2>/dev/null || md5sum "$BIN" | awk '{print $1}')
scp_do "$BIN" "$REMOTE_DIR/sockrocket-cli.new"
scp_do "$ASP" "$REMOTE_DIR/webui/sockrocket.asp"
scp_do "$ASP" "/www/ext/sockrocket/sockrocket.asp"
# Normalize LF for ash
tr -d '\r' < "$SH" > /tmp/sockrocket.sh.lf
scp_do /tmp/sockrocket.sh.lf "$REMOTE_DIR/scripts/sockrocket.sh"
rm -f /tmp/sockrocket.sh.lf

REMOTE_MD5=$(ssh_do "md5sum $REMOTE_DIR/sockrocket-cli 2>/dev/null | awk '{print \$1}'" || true)
NEW_MD5=$(ssh_do "md5sum $REMOTE_DIR/sockrocket-cli.new | awk '{print \$1}'")
echo "    local=$LOCAL_MD5"
echo "    remote_running=${REMOTE_MD5:-unknown}"
echo "    remote_new=$NEW_MD5"
[[ "$NEW_MD5" == "$LOCAL_MD5" ]] || { echo "upload md5 mismatch"; exit 1; }

NEED_RESTART=1
if [[ "${SKIP_RESTART:-0}" == "1" ]]; then
  NEED_RESTART=0
elif [[ "${FORCE_RESTART:-0}" != "1" && -n "$REMOTE_MD5" && "$REMOTE_MD5" == "$NEW_MD5" ]]; then
  echo "==> binary unchanged — skip daemon restart (avoid network flap)"
  ssh_do "rm -f $REMOTE_DIR/sockrocket-cli.new; chmod +x $REMOTE_DIR/scripts/sockrocket.sh"
  NEED_RESTART=0
fi

if [[ "$NEED_RESTART" == "1" ]]; then
  echo "==> 2/4 schedule detached swap+restart (SSH may drop)"
  # Run fully detached; give SSH a moment to return the ack before links die.
  ssh_do "nohup sh -c '
    exec >>/tmp/sockrocket-deploy.log 2>&1
    echo \"=== deploy \$(date) ===\"
    cd $REMOTE_DIR || exit 1
    chmod +x scripts/sockrocket.sh sockrocket-cli.new
    sh scripts/sockrocket.sh stop
    mv -f sockrocket-cli.new sockrocket-cli
    chmod +x sockrocket-cli
    sleep 2
    sh scripts/sockrocket.sh start
    sh scripts/sockrocket.sh api-restart
    sleep 2
    sh scripts/sockrocket.sh status
    echo DEPLOY_OK
  ' >/dev/null 2>&1 &
  echo DETACHED_PID=\$!
  sleep 1
  echo ACK"
  echo "==> 3/4 wait for SSH to return (up to ~90s)"
  sleep 8
  ok=0
  for i in $(seq 1 30); do
    if nc -z -G 3 "$HOST" "$PORT" 2>/dev/null; then
      if ssh_do "echo UP; test -x $REMOTE_DIR/sockrocket-cli; ! test -e $REMOTE_DIR/sockrocket-cli.new" 2>/dev/null; then
        ok=1
        break
      fi
    fi
    echo "    wait $i …"
    sleep 3
  done
  [[ "$ok" == "1" ]] || {
    echo "SSH did not return cleanly; check router console /tmp/sockrocket-deploy.log"
    exit 1
  }
else
  echo "==> 2-3/4 skipped restart"
fi

echo "==> 4/4 verify"
ssh_do "
  sh $REMOTE_DIR/scripts/sockrocket.sh status | head -12
  curl -s -m 5 http://127.0.0.1:18188/?action=version
  echo
  curl -s -o /dev/null -w 'socks=%{http_code} t=%{time_total}\n' --connect-timeout 8 --max-time 12 --socks5-hostname 127.0.0.1:1080 http://www.gstatic.com/generate_204
  # Hot-reload must not re-fetch subscriptions
  before=\$(wc -c < $REMOTE_DIR/sockrocket.log)
  curl -s -m 5 -X POST http://127.0.0.1:18188/?action=set_node -H 'Content-Type: application/json' -d '{\"index\":20}'
  echo
  sleep 4
  echo ---reload---
  tail -c +\$((before+1)) $REMOTE_DIR/sockrocket.log | grep -E 'Fetching|config reloaded|active=' | tail -8
"
echo "DONE"
