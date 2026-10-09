#!/bin/sh
# Safe router-side probe: turn TUN off then on and measure Google recovery.
#
# ALWAYS restores transparent_proxy + dns_hijack + restarts on EXIT so a
# failed/interrupted run cannot leave the LAN without proxy/DNS.
#
# Usage (on the router, short SSH session):
#   sh /jffs/addons/sockrocket/scripts/verify_tun_toggle.sh
# Or copy to /tmp and run from there.
#
# Success: after re-enable, google.com resolves to Fake-IP (198.18.x.x) and
# HTTPS via SOCKS returns 204 within GOAL_SECS (default 10).
set -u

SOCKROCKET_DIR="${SOCKROCKET_DIR:-/jffs/addons/sockrocket}"
CONF="${SOCKROCKET_DIR}/config.yaml"
SH="${SOCKROCKET_DIR}/scripts/sockrocket.sh"
GOAL_SECS="${GOAL_SECS:-10}"
SOCKS_PORT="${SOCKS_PORT:-1080}"
DNS_CONF="${DNS_CONF:-/jffs/configs/dnsmasq.d/sockrocket.conf}"

log() { printf '[verify] %s\n' "$*"; }
die() { log "FAIL: $*"; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

need nslookup
need curl
need date
[ -f "$CONF" ] || die "missing $CONF"
[ -x "$SH" ] || die "missing $SH"

ensure_key() {
    # $1=key $2=value — upsert a top-level yaml bool key.
    local key="$1" val="$2"
    if grep -q "^${key}:" "$CONF" 2>/dev/null; then
        sed -i "s/^${key}:.*/${key}: ${val}/" "$CONF"
    else
        echo "${key}: ${val}" >> "$CONF"
    fi
}

restore() {
    log "RESTORE: transparent_proxy=true dns_hijack=true + restart + sshd"
    ensure_key transparent_proxy true
    ensure_key dns_hijack true
    "$SH" restart >/dev/null 2>&1 || "$SH" start >/dev/null 2>&1 || true
    "$SH" ensure-sshd >/dev/null 2>&1 || true
}
trap restore EXIT INT TERM HUP

lookup_a() {
    local host="$1" server="${2:-}"
    if [ -n "$server" ]; then
        nslookup "$host" "$server" 2>/dev/null | awk '/^Address:/{addr=$2} END{print addr}'
    else
        nslookup "$host" 2>/dev/null | awk '/^Address:/{addr=$2} END{print addr}'
    fi
}

is_fakeip() {
    case "$1" in
        198.18.*) return 0 ;;
        *) return 1 ;;
    esac
}

# ── 0. Baseline: ensure TUN+DNS on ──────────────────────────────────────────
log "baseline: force TUN+DNS on"
ensure_key transparent_proxy true
ensure_key dns_hijack true
"$SH" restart >/dev/null 2>&1 || die "baseline restart failed"
sleep 2

# ── 1. Turn TUN off (keep DNS) ──────────────────────────────────────────────
log "step1: proxy-off (expect DNS hijack kept, real IP — not Fake-IP)"
ensure_key transparent_proxy false
# Intentionally do NOT clear dns_hijack — that is the bug this catches.
"$SH" proxy-off >/dev/null 2>&1 || die "proxy-off failed"
sleep 2

[ -f "$DNS_CONF" ] || die "dns hijack file missing after proxy-off (must stay)"

IP_OFF=$(lookup_a google.com 127.0.0.1)
[ -n "$IP_OFF" ] || IP_OFF=$(lookup_a google.com)
[ -n "$IP_OFF" ] || die "no A for google.com after proxy-off"
if is_fakeip "$IP_OFF"; then
    die "still Fake-IP ($IP_OFF) after proxy-off — Fake-IP requires --tun"
fi
log "ok: google.com -> $IP_OFF (real IP, not Fake-IP)"

# ── 2. Turn TUN back on ─────────────────────────────────────────────────────
log "step2: re-enable TUN+DNS, poll until Fake-IP + SOCKS 204 (goal ≤${GOAL_SECS}s)"
ensure_key transparent_proxy true
ensure_key dns_hijack true
t0=$(date +%s)
"$SH" restart >/dev/null 2>&1 || die "re-enable restart failed"

ok=0
elapsed=0
IP_ON=""
code="000"
while [ "$elapsed" -le "$GOAL_SECS" ]; do
    IP_ON=$(lookup_a google.com 127.0.0.1)
    [ -n "$IP_ON" ] || IP_ON=$(lookup_a google.com)
    socks_ok=0
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 3 \
        --socks5-hostname 127.0.0.1:"$SOCKS_PORT" \
        http://www.gstatic.com/generate_204 2>/dev/null || echo 000)
    [ "$code" = "204" ] || [ "$code" = "200" ] && socks_ok=1

    if is_fakeip "$IP_ON" && [ "$socks_ok" = "1" ]; then
        ok=1
        break
    fi
    sleep 1
    now=$(date +%s)
    elapsed=$((now - t0))
done

if [ "$ok" = "1" ]; then
    log "PASS: Fake-IP=$IP_ON SOCKS=$code in ${elapsed}s (goal ≤${GOAL_SECS}s)"
    exit 0
fi
die "Google not ready in ${GOAL_SECS}s (last IP=${IP_ON:-none} socks=${code:-none} elapsed=${elapsed}s)"
