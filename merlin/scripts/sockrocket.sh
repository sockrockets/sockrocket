#!/bin/sh
# Sockrocket proxy service control script for AsusWRT-Merlin
#
# Usage: sockrocket.sh {start|stop|restart|reload|status|log|diagnose|update-subs|watchdog|api-start|api-stop|api-restart}

SOCKROCKET_DIR="/jffs/addons/sockrocket"
SOCKROCKET_BIN="$SOCKROCKET_DIR/sockrocket-cli"
SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
SOCKROCKET_SCRIPTS="$SOCKROCKET_DIR/scripts"
SOCKROCKET_PID="$SOCKROCKET_DIR/sockrocket.pid"
SOCKROCKET_LOG="$SOCKROCKET_DIR/sockrocket.log"
SOCKROCKET_LOCK="$SOCKROCKET_DIR/sockrocket.lock"
# Crash forensics ("black box"): text history on jffs (tiny). Optional core
# dumps live on /tmp (tmpfs) and are OFF by default — unlimited cores plus a
# system-wide core_pattern rewrite can fill router RAM and affect other
# processes. Set SOCKROCKET_CORE_DUMPS=1 to enable.
SOCKROCKET_CRASH_LOG="$SOCKROCKET_DIR/crash-history.log"
SOCKROCKET_CORE_DIR="/tmp"
SOCKROCKET_CORE_PATTERN="/tmp/sockrocket-core.%p"
SOCKROCKET_CORE_PATTERN_BAK="$SOCKROCKET_DIR/.core_pattern.bak"
SOCKROCKET_CORE_DUMPS="${SOCKROCKET_CORE_DUMPS:-0}"
CRASH_LOG_MAX_BYTES=204800   # 200 KB of text history, then keep the tail
SOCKROCKET_DEBUG="${SOCKROCKET_DEBUG:-0}"  # Set SOCKROCKET_DEBUG=1 to enable debug logging
LOG_MAX_BYTES=524288   # 512 KB
CORE_KEEP_MAX=1        # at most one gzipped core on /tmp

# /proc prefix for the liveness checks below. Overridable only so the test
# suite can emulate /proc/<pid>/comm on a dev box where that file does not
# exist (MSYS/Windows); production never sets SOCKROCKET_PROC_BASE.
PROC_BASE="${SOCKROCKET_PROC_BASE:-/proc}"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
log_info()  { logger -t sockrocket -p daemon.info "$*"; printf "${CYAN}[Sockrocket] %s${NC}\n" "$*"; }
log_ok()    { logger -t sockrocket -p daemon.info "$*"; printf "${GREEN}[Sockrocket] ✓ %s${NC}\n" "$*"; }
log_warn()  { logger -t sockrocket -p daemon.warn "$*"; printf "${YELLOW}[Sockrocket] ! %s${NC}\n" "$*" >&2; }
log_error() { logger -t sockrocket -p daemon.err "$*"; printf "${RED}[Sockrocket] ✗ %s${NC}\n" "$*" >&2; }
log_debug() { [ "${SOCKROCKET_DEBUG:-0}" = "1" ] && logger -t sockrocket -p daemon.debug "$*"; }

# Backward-compatible aliases
info()  { log_info "$*"; }
ok()    { log_ok "$*"; }
warn()  { log_warn "$*"; }
err()   { log_error "$*"; }

is_running() {
    [ -f "$SOCKROCKET_PID" ] || return 1
    local pid
    pid=$(cat "$SOCKROCKET_PID")
    kill -0 "$pid" 2>/dev/null || return 1
    # Verify the PID belongs to sockrocket-cli, not a reused PID
    grep -q "sockrocket-cli" "$PROC_BASE/$pid/comm" 2>/dev/null || return 1
    return 0
}

# ── Orphan daemon cleanup ─────────────────────────────────────────────────────
# PIDs of sockrocket-cli DAEMON processes, found by full command line rather than the
# pid file. The API bridge (`sockrocket-cli api <port>`) never matches: its cmdline
# has no config path. When the pid file lies (stale entry after a crash, or a
# start that raced another start), a daemon it doesn't mention keeps serving
# traffic on a stale config while every new start dies with EADDRINUSE — the
# router hit exactly this: status said "stopped", yet an old daemon held
# 1080/1087/5300 and the watchdog respawned a dying duplicate every 5 minutes.
daemon_pids() {
    ps w 2>/dev/null | grep "[s]ockrocket-cli $SOCKROCKET_CONF" | awk '$1 ~ /^[0-9]+$/ {print $1}'
}

api_pids() {
    ps w 2>/dev/null | grep "[s]ockrocket-cli api" | awk '$1 ~ /^[0-9]+$/ {print $1}'
}

# Kill every daemon the pid file doesn't track, waiting for them to exit.
# Best effort: after the kill -9 fallback we still wait briefly so start does
# not race EADDRINUSE on 1080/1087/5300.
kill_stray_daemons() {
    local pids left wait
    pids=$(daemon_pids)
    [ -z "$pids" ] && return 0
    warn "Found unregistered Sockrocket daemon(s) (PID: $pids) -- cleaning up"
    for p in $pids; do kill "$p" 2>/dev/null || true; done
    wait=0
    while [ "$wait" -lt 5 ]; do
        left=$(daemon_pids)
        [ -z "$left" ] && return 0
        sleep 1; wait=$((wait + 1))
    done
    left=$(daemon_pids)
    for p in $left; do kill -9 "$p" 2>/dev/null || true; done
    wait=0
    while [ "$wait" -lt 3 ]; do
        left=$(daemon_pids)
        [ -z "$left" ] && return 0
        sleep 1; wait=$((wait + 1))
    done
}

kill_stray_apis() {
    local keep pids p left wait
    keep=$(cat "$API_PID" 2>/dev/null)
    pids=$(api_pids)
    [ -z "$pids" ] && return 0
    for p in $pids; do
        [ -n "$keep" ] && [ "$p" = "$keep" ] && continue
        warn "Found unregistered Sockrocket API (PID: $p) -- cleaning up"
        kill "$p" 2>/dev/null || true
    done
    wait=0
    while [ "$wait" -lt 3 ]; do
        left=
        for p in $(api_pids); do
            [ -n "$keep" ] && [ "$p" = "$keep" ] && continue
            left="$left $p"
        done
        [ -z "$(echo $left)" ] && return 0
        sleep 1; wait=$((wait + 1))
    done
    for p in $(api_pids); do
        [ -n "$keep" ] && [ "$p" = "$keep" ] && continue
        kill -9 "$p" 2>/dev/null || true
    done
}

# ── Web UI API bridge (always-on, independent of the proxy daemon) ──────────
# The router's httpd cannot execute user CGIs (/cgi-bin/sockrocket.cgi always 404s
# on stock Merlin), so the Web UI talks to `sockrocket-cli api` — a tiny HTTP JSON
# server on port 18188. It must stay up even while the proxy is stopped,
# otherwise the UI has no way to start the service again.
API_PID="$SOCKROCKET_DIR/sockrocket-api.pid"
DEFAULT_API_PORT=18188

# api_port from config.yaml, default 18188. Read at runtime (not parse
# time) so config edits take effect on the next api-start.
api_port() {
    local p
    p=$(get_config_value api_port 2>/dev/null)
    case "$p" in ''|*[!0-9]*) echo "$DEFAULT_API_PORT" ;; *) echo "$p" ;; esac
}

# Publish the API port next to the Web UI page: the .asp is static and
# cannot read config.yaml, so the browser learns the port from api.js.
write_api_js() {
    [ -d /www/ext/sockrocket ] || return 0
    printf 'var SOCKROCKET_API_PORT = %s;\n' "$1" > /www/ext/sockrocket/api.js 2>/dev/null || true
}

api_is_running() {
    [ -f "$API_PID" ] || return 1
    local pid
    pid=$(cat "$API_PID")
    kill -0 "$pid" 2>/dev/null || return 1
    grep -q "sockrocket-cli" "$PROC_BASE/$pid/comm" 2>/dev/null || return 1
    return 0
}

do_api_start() {
    api_is_running && return 0
    [ -x "$SOCKROCKET_BIN" ] || return 1
    # Clear orphans that hold :18188 before we bind.
    kill_stray_apis
    local port
    port=$(api_port)
    # Slightly lower priority than LAN forwarding / dnsmasq so Web UI probes
    # cannot starve the data plane on a busy ARM core.
    nice -n 5 "$SOCKROCKET_BIN" api "$port" >> "$SOCKROCKET_LOG" 2>&1 &
    echo $! > "$API_PID"
    write_api_js "$port"
    sleep 1
    if api_is_running; then
        log_info "API bridge started (PID $(cat "$API_PID"), port $port)"
    else
        log_warn "API bridge failed to start"
        rm -f "$API_PID"
        return 1
    fi
}

do_api_stop() {
    if ! api_is_running; then rm -f "$API_PID"; return 0; fi
    local pid
    pid=$(cat "$API_PID")
    kill "$pid" 2>/dev/null
    sleep 1
    kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    rm -f "$API_PID"
    log_info "API bridge stopped"
}

# ── Lock (atomic mkdir-based, no TOCTOU race) ───────────────────────────────
LOCK_DIR="${SOCKROCKET_DIR}/.lockdir"
LOCK_HELD=0   # 1 only while THIS process holds the lock

acquire_lock() {
    local timeout="${1:-15}"
    while [ "$timeout" -gt 0 ]; do
        mkdir "$LOCK_DIR" 2>/dev/null && { echo $$ > "$LOCK_DIR/pid"; LOCK_HELD=1; return 0; }
        # Stale-lock recovery: if the holder PID is dead, break the lock.
        local holder
        holder=$(cat "$LOCK_DIR/pid" 2>/dev/null)
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            log_warn "Breaking stale lock (holder PID $holder is dead)"
            rm -rf "$LOCK_DIR" 2>/dev/null
            continue
        fi
        timeout=$((timeout - 1))
        sleep 1   # busybox sleep on this firmware rejects fractional seconds
    done
    log_error "Lock acquire timeout after 15s — another operation may be stuck"
    return 1
}

release_lock() {
    # Only the process that actually acquired the lock may release it.
    # The EXIT trap also fires for unlocked actions (status/log/diagnose);
    # without this guard a concurrent `status` poll would delete the pid
    # file and rmdir the lock right out from under a running start/stop.
    [ "$LOCK_HELD" = "1" ] || return 0
    LOCK_HELD=0
    rm -f "$LOCK_DIR/pid" 2>/dev/null
    rmdir "$LOCK_DIR" 2>/dev/null || true
}

# ── Log rotation ──────────────────────────────────────────────────────────────
rotate_log() {
    [ -f "$SOCKROCKET_LOG" ] || return
    local size
    size=$(wc -c < "$SOCKROCKET_LOG" 2>/dev/null || echo 0)
    if [ "$size" -gt "$LOG_MAX_BYTES" ]; then
        tail -c "$((LOG_MAX_BYTES / 2))" "$SOCKROCKET_LOG" > "$SOCKROCKET_LOG.tmp" \
            && mv "$SOCKROCKET_LOG.tmp" "$SOCKROCKET_LOG"
        info "Log rotated (was ${size} bytes)"
    fi
}

# ── Cron integration ──────────────────────────────────────────────────────────
setup_cron() {
    # Subscription auto-update daily at 04:00
    cru a SockrocketSubUpdate "0 4 * * * $SOCKROCKET_DIR/scripts/sockrocket.sh update-subs" 2>/dev/null || true
    # Watchdog every 5 minutes
    cru a SockrocketWatchdog "*/5 * * * * $SOCKROCKET_DIR/scripts/sockrocket.sh watchdog" 2>/dev/null || true
}

remove_cron() {
    cru d SockrocketSubUpdate 2>/dev/null || true
    cru d SockrocketWatchdog  2>/dev/null || true
    cru d SockrocketBoot      2>/dev/null || true
}

# ── DNS config ────────────────────────────────────────────────────────────────
get_config_value() {
    grep "^${1}:" "$SOCKROCKET_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'
}

# Count nodes / subscriptions in config.yaml (0 when the section is [] or absent)
count_section() {
    awk -v sec="$1" '
        $0 ~ "^" sec ":" { in_s=1; is_empty=($0 ~ /\[\]/); next }
        in_s && /^[a-zA-Z_]/ { in_s=0 }
        in_s && /^[[:space:]]*- / { c++ }
        END { print (is_empty ? 0 : c+0) }
    ' "$SOCKROCKET_CONF" 2>/dev/null || echo 0
}

# Ensure the two toggle keys exist in config.yaml, deriving them from the
# legacy `mode` key when absent. Idempotent; called from do_start.
migrate_toggle_keys() {
    [ -f "$SOCKROCKET_CONF" ] || return 0
    [ -n "$(get_config_value transparent_proxy)" ] && [ -n "$(get_config_value dns_hijack)" ] && return 0
    local mode tp dh
    mode=$(get_config_value mode)
    case "${mode:-tun}" in
        tun) tp=true;  dh=true  ;;
        *)   tp=false; dh=false ;;
    esac
    local tmp="$SOCKROCKET_CONF.tmp"
    {
        grep -q "^transparent_proxy:" "$SOCKROCKET_CONF" || echo "transparent_proxy: $tp"
        grep -q "^dns_hijack:"        "$SOCKROCKET_CONF" || echo "dns_hijack: $dh"
        cat "$SOCKROCKET_CONF"
    } > "$tmp" && mv "$tmp" "$SOCKROCKET_CONF"
}

# Pin dnsmasq's upstream server file (servers-file) to Sockrocket's DNS listener.
#
# The firmware generates /tmp/resolv.dnsmasq with the ISP's DNS servers.
# dnsmasq load-balances queries across ALL configured servers and favours the
# fastest responder — so the ISP's GFW-poisoned answers (google.com →
# poisoned A/AAAA records) consistently beat the clean proxied ones from the
# Sockrocket listener. Overwriting the file and SIGHUPing dnsmasq leaves the Sockrocket
# listener as the ONLY upstream. The firmware regenerates the file whenever
# dnsmasq restarts, the WAN reconfigures, or the hijack is removed, so the
# default behaviour self-restores outside hijack mode.
pin_dns_upstream() {
    local dns_port dns_conf="${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf"
    local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
    local bak_file="${resolv_file}.Sockrocket-bak"
    [ -f "$dns_conf" ] || return 0
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    # Only pin while our listener is actually answering.
    netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)" || return 0
    # Skip the rewrite (and the SIGHUP) when already pinned.
    [ "$(cat "$resolv_file" 2>/dev/null)" = "server=127.0.0.1#${dns_port}" ] && return 0
    # Back up the firmware's ISP upstreams BEFORE the first overwrite — the
    # stop path restores them (see unpin_dns_upstream). Never snapshot our own
    # pin: a restart while pinned would "restore" a dead upstream forever.
    grep -q "^server=127.0.0.1#" "$resolv_file" 2>/dev/null || \
        cp "$resolv_file" "$bak_file" 2>/dev/null || true
    echo "server=127.0.0.1#${dns_port}" > "$resolv_file"
    kill -HUP "$(pidof dnsmasq)" 2>/dev/null
    ok "dnsmasq upstream pinned to 127.0.0.1#${dns_port} (LAN queries no longer go through ISP DNS)"
}

# Restore dnsmasq's original upstream servers when the hijack is removed.
# The comment block above assumed the firmware REGENERATES resolv.dnsmasq on
# dnsmasq restart — on current Merlin it does NOT (verified: after
# `update_dnsmasq stop` the file still held our pin), so without this unpin
# every Sockrocket stop left dnsmasq pointing at a dead 127.0.0.1:<port> and the
# whole LAN lost DNS ("nothing is reachable after enabling/disabling").
unpin_dns_upstream() {
    local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
    local bak_file="${resolv_file}.Sockrocket-bak"
    # Only act when the file currently holds our pin (someone else's content
    # — e.g. freshly regenerated by the firmware — must be left alone).
    grep -q "^server=127.0.0.1#" "$resolv_file" 2>/dev/null || return 0
    if [ -s "$bak_file" ]; then
        cp "$bak_file" "$resolv_file"
        rm -f "$bak_file"
    else
        # No backup (pinned by an older build): rebuild from the system
        # resolver, skipping loopback entries that would loop dnsmasq into
        # itself. Skip the write entirely if nothing usable was found.
        local rebuilt
        rebuilt=$(awk '/^nameserver/ && $2 != "127.0.0.1" && $2 != "::1" {print "server=" $2}' \
            "${SOCKROCKET_RESOLV_CONF:-/etc/resolv.conf}" 2>/dev/null)
        [ -n "$rebuilt" ] && printf '%s\n' "$rebuilt" > "$resolv_file"
    fi
    kill -HUP "$(pidof dnsmasq)" 2>/dev/null
    ok "dnsmasq upstream restored to ISP DNS"
}

update_dnsmasq() {
    local action="$1"
    # Overridable for tests/integration/sockrocket_sh_test.sh (default stays the firmware path).
    local DNSMASQ_CONF_DIR="${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}"
    local SOCKROCKET_DNSMASQ="$DNSMASQ_CONF_DIR/sockrocket.conf"
    local TEMPLATE="$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"

    mkdir -p "$DNSMASQ_CONF_DIR"

    if [ "$action" = "start" ] && [ -f "$TEMPLATE" ]; then        local dns_port router_ip dns_server
        dns_port=$(get_config_value dns_port)
        dns_port=${dns_port:-5300}
        # Fail-safe: only hijack default DNS when Sockrocket's DNS listener is
        # actually up. The template's catch-all upstream would otherwise
        # blackhole every LAN lookup, taking the whole network offline.
        # Matches ":port " (followed by the peer column) or ":port" at
        # end-of-line, since busybox netstat column spacing varies.
        if ! netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)"; then
            rm -f "$SOCKROCKET_DNSMASQ"
            warn "Sockrocket DNS port $dns_port not listening -- DNS split not enabled (LAN DNS unchanged)"
            return 0
        fi
        router_ip=$(nvram get lan_ipaddr 2>/dev/null)
        # nvram exits 0 with an empty value for unset keys — the `||`
        # fallback would not trigger, so default on empty explicitly.
        [ -n "$router_ip" ] || router_ip="192.168.1.1"
        sed -e "s/__DNS_PORT__/$dns_port/g" \
            -e "s/__ROUTER_IP__/$router_ip/g" \
            "$TEMPLATE" > "$SOCKROCKET_DNSMASQ"
        # NOTE: the old dns_direct_domains bypass (dnsmasq server=/ entries
        # straight to the domestic resolver) was removed: it
        # short-circuited Sockrocket's resolver, so those names never entered the
        # domestic-IP table (geoip misses then misrouted their bare-IP
        # traffic into the proxy — a domestic-site misroute) and user routing
        # rules could never match them. ALL queries now go through Sockrocket's
        # listener, whose china-split serves domestic names from
        # 223.5.5.5/119.29.29.29 itself.
        if service restart_dnsmasq >/dev/null 2>&1; then
            pin_dns_upstream
            # Also force-capture LAN DNS that bypasses the router entirely
            # (devices hard-coded to 8.8.8.8 & friends — GFW poisons those
            # UDP answers on the wire). Redirect every LAN :53 query into
            # this dnsmasq so the china-split above applies to them too.
            [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start >/dev/null 2>&1 || true
            ok "dnsmasq rules applied (DNS port: $dns_port, all queries via Sockrocket listener)"
        else
            log_error "dnsmasq restart FAILED — DNS may be broken! Check config with: dnsmasq --test"
            log_warn "dnsmasq config written but restart failed (DNS port: $dns_port)"
        fi
    elif [ "$action" = "stop" ]; then
        rm -f "$SOCKROCKET_DNSMASQ"
        [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
            "$SOCKROCKET_DIR/scripts/iptables.sh" dns-stop >/dev/null 2>&1 || true
        # Unpin BEFORE restarting dnsmasq, so the restarted daemon reads the
        # restored ISP upstreams instead of our now-dead listener.
        unpin_dns_upstream
        service restart_dnsmasq 2>/dev/null && ok "dnsmasq rules removed" || log_warn "dnsmasq restart failed during cleanup"
    fi
}

# ── Start ─────────────────────────────────────────────────────────────────────
do_start() {
    if is_running; then
        warn "Sockrocket is already running (PID $(cat "$SOCKROCKET_PID"))"
        return 0
    fi

    [ -x "$SOCKROCKET_BIN" ]  || { err "Binary not found: $SOCKROCKET_BIN"; return 1; }
    [ -f "$SOCKROCKET_CONF" ] || { err "Config not found: $SOCKROCKET_CONF"; return 1; }

    # The pid file may point at a dead process while a daemon started outside
    # it (raced start, crashed intermediate) still holds the proxy ports.
    # Launching over that orphan ends with EADDRINUSE and a pid file that
    # tracks a corpse, so clear strays BEFORE forking the new daemon.
    kill_stray_daemons

    # Starting clears the user's stop intent, re-arming the watchdog.
    rm -f "$SOCKROCKET_DIR/stopped"

    # The Web UI's API bridge must run regardless of proxy state.
    do_api_start

    # Restore CGI symlink (tmpfs /www/cgi-bin loses files on reboot).
    # Skipped automatically on koolcenter firmware where /www is read-only
    # (mkdir fails) — the koolcenter path uses the API bridge instead.
    local cgi_src="$SOCKROCKET_DIR/sockrocket.cgi"
    local cgi_dst="/www/cgi-bin/sockrocket.cgi"
    if [ -f "$cgi_src" ] && [ ! -e "$cgi_dst" ]; then
        if mkdir -p "/www/cgi-bin" 2>/dev/null; then
            ln -sf "$cgi_src" "$cgi_dst" 2>/dev/null || cp "$cgi_src" "$cgi_dst" 2>/dev/null || true
        fi
    fi

    # Same story for the Web UI page on standard Merlin: if /www was
    # rebuilt on reboot, /www/ext/sockrocket is gone — restore page + api.js.
    if [ ! -f /www/ext/sockrocket/sockrocket.asp ] && [ -f "$SOCKROCKET_DIR/webui/sockrocket.asp" ]; then
        if mkdir -p /www/ext/sockrocket 2>/dev/null; then
            cp "$SOCKROCKET_DIR/webui/sockrocket.asp" /www/ext/sockrocket/sockrocket.asp 2>/dev/null || true
        fi
    fi
    write_api_js "$(api_port)"

    # koolcenter self-heal: the offline installer's sandbox can miss the
    # web page / API bridge / dbus registration; reinstall them here from
    # the normal runtime environment on every start.
    if [ -d /koolshare/scripts ] && [ -f /koolshare/scripts/base.sh ]; then
        local KS_WEBS="/koolshare/webs"
        if [ ! -f "$KS_WEBS/Module_sockrocket.asp" ] && [ -f "$SOCKROCKET_DIR/webui/sockrocket.asp" ]; then
            cp "$SOCKROCKET_DIR/webui/sockrocket.asp" "$KS_WEBS/Module_sockrocket.asp" 2>/dev/null || true
        fi
        if [ ! -f /koolshare/res/icon-sockrocket.png ]; then
            if [ -f "$SOCKROCKET_DIR/res/icon-sockrocket.png" ]; then
                mkdir -p /koolshare/res 2>/dev/null || true
                cp "$SOCKROCKET_DIR/res/icon-sockrocket.png" /koolshare/res/icon-sockrocket.png 2>/dev/null || true
            fi
        fi
        if [ ! -f /koolshare/scripts/sockrocket_api.sh ] && [ -f "$SOCKROCKET_SCRIPTS/sockrocket_api.sh" ]; then
            cp "$SOCKROCKET_SCRIPTS/sockrocket_api.sh" /koolshare/scripts/sockrocket_api.sh 2>/dev/null
            chmod +x /koolshare/scripts/sockrocket_api.sh 2>/dev/null || true
        fi
        if which dbus >/dev/null 2>&1; then
            [ -z "$(dbus get softcenter_module_sockrocket_install 2>/dev/null)" ] && \
                dbus set softcenter_module_sockrocket_install=1 2>/dev/null || true
            [ -z "$(dbus get softcenter_module_sockrocket_title 2>/dev/null)" ] && \
                dbus set softcenter_module_sockrocket_title="Sockrocket" 2>/dev/null || true
            [ -z "$(dbus get softcenter_module_sockrocket_name 2>/dev/null)" ] && \
                dbus set softcenter_module_sockrocket_name="Sockrocket" 2>/dev/null || true
            [ -z "$(dbus get softcenter_module_sockrocket_description 2>/dev/null)" ] && \
                dbus set softcenter_module_sockrocket_description="Transparent proxy (TUN/SOCKS5/HTTP)" 2>/dev/null || true
            [ -z "$(dbus get softcenter_module_sockrocket_indexpage 2>/dev/null)" ] && \
                dbus set softcenter_module_sockrocket_indexpage="Module_sockrocket.asp" 2>/dev/null || true
            local VER
            VER=$(cat "$SOCKROCKET_DIR/version" 2>/dev/null)
            [ -n "$VER" ] && dbus set softcenter_module_sockrocket_version="$VER" 2>/dev/null || true
        fi
    fi

    rotate_log
    info "Starting Sockrocket..."

    # TUN kernel module: the firmware ships tun.ko but does not auto-load it,
    # so /dev/net/tun is missing after every reboot until we do this.
    # NOTE: this busybox (v1.25.1) has NO `command` builtin — `command -v X`
    # always fails here, silently skipping whatever it guarded. `which` is an
    # applet/binary on every AsusWRT build, so probe with that instead.
    if [ ! -e /dev/net/tun ] && which modprobe >/dev/null 2>&1; then
        modprobe tun 2>/dev/null && log_info "Loaded tun kernel module" \
            || log_warn "modprobe tun failed — TUN mode unavailable"
    fi

    # Zero-config / not-ready guard: TUN + DNS hijack must not run until
    # there is at least one persisted node. A subscription URL alone is not
    # enough — fetch may still fail, and hijacking LAN DNS in that state
    # makes Softcenter show "数据加载中，请稍候重试" and can blackhole SSH.
    local force_socks=0
    if [ "$(count_section nodes)" = "0" ]; then
        force_socks=1
        warn "No nodes in config -- starting in plain SOCKS/HTTP mode (transparent proxy and DNS hijack skipped)"
        warn "Add a subscription or node in the Web UI first, then restart the service"
    fi

    # Transparent proxy is driven by its own config key, not by `mode`
    # (mode is kept in sync for backwards compatibility with sockrocket-cli itself).
    migrate_toggle_keys
    local want_proxy want_dns
    want_proxy=$(conf_bool transparent_proxy true)
    want_dns=$(conf_bool dns_hijack true)

    if [ "$force_socks" = "1" ]; then
        if [ "$want_proxy" = "true" ]; then
            want_proxy=false
            warn "No usable nodes -- transparent proxy skipped (SOCKS/HTTP ports only)"
        fi
        if [ "$want_dns" = "true" ]; then
            want_dns=false
            warn "No usable nodes -- DNS hijack skipped until nodes are available"
        fi
    fi

    # TUN must be up BEFORE handing --tun to sockrocket-cli.
    if [ "$want_proxy" = "true" ] && ! ensure_tun; then
        log_error "Failed to enable transparent proxy: TUN device unavailable (tried modprobe/insmod/mknod)"
        want_proxy=false
    fi

    # Core dumps must be enabled in THIS shell: the daemon inherits ulimit
    # from its parent. core_pattern is system-wide and idempotent.
    enable_core_dumps

    if [ "$want_proxy" = "true" ]; then
        nice -n 3 "$SOCKROCKET_BIN" "$SOCKROCKET_CONF" --tun >> "$SOCKROCKET_LOG" 2>&1 &
        [ "$(get_config_value mode)" != "tun" ] && sed -i 's/^mode:.*/mode: "tun"/' "$SOCKROCKET_CONF" 2>/dev/null
    else
        nice -n 3 "$SOCKROCKET_BIN" "$SOCKROCKET_CONF" >> "$SOCKROCKET_LOG" 2>&1 &
        [ "$(get_config_value mode)" != "socks" ] && sed -i 's/^mode:.*/mode: "socks"/' "$SOCKROCKET_CONF" 2>/dev/null
    fi
    echo $! > "$SOCKROCKET_PID"
    sleep 1

    if is_running; then
        ok "Sockrocket started (PID $(cat "$SOCKROCKET_PID"))"
        # Softcenter offline install must not block 20–35s on TUN/DNS waits —
        # that freezes the software-center UI ("数据加载中"). FAST_START
        # applies rules in the background; normal starts wait as before.
        if [ "${SOCKROCKET_FAST_START:-0}" = "1" ]; then
            (
                trap '' HUP INT TERM
                if [ "$want_proxy" = "true" ]; then
                    if wait_for_tun 20; then
                        [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                            "$SOCKROCKET_DIR/scripts/iptables.sh" start >> "$SOCKROCKET_LOG" 2>&1
                    fi
                fi
                if [ "$want_dns" = "true" ]; then
                    if wait_for_dns 15; then
                        update_dnsmasq start
                    fi
                elif [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]; then
                    update_dnsmasq stop
                fi
                if [ "$want_proxy" != "true" ] && \
                    iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                        "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
                fi
            ) >> "$SOCKROCKET_LOG" 2>&1 &
        else
            if [ "$want_proxy" = "true" ]; then
                if wait_for_tun 20; then
                    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" start >> "$SOCKROCKET_LOG" 2>&1
                else
                    log_warn "TUN device not ready within 20s -- transparent proxy not enabled (check the log)"
                fi
            fi
            if [ "$want_dns" = "true" ]; then
                if wait_for_dns 15; then
                    update_dnsmasq start
                else
                    log_warn "DNS port 5300 not listening within 15s -- DNS split not enabled (LAN DNS unchanged)"
                fi
            elif [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]; then
                update_dnsmasq stop
            fi
            if [ "$want_proxy" != "true" ] && \
                iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
            fi
        fi
        setup_cron
    else
        log_error "Sockrocket failed to start. Last log lines:"
        tail -20 "$SOCKROCKET_LOG" 2>/dev/null | while IFS= read -r line; do
            log_error "  $line"
        done
        rm -f "$SOCKROCKET_PID"
        return 1
    fi
}

# Device node path for TUN. Overridable so tests can point at a marker file
# (the real path cannot be created on a dev box).
TUN_DEV="${SOCKROCKET_TUN_DEV:-/dev/net/tun}"

# Read a boolean config key with a default. Accepts YAML true/false and the
# quoted forms; anything else falls back to $2.
conf_bool() {
    local v
    v=$(get_config_value "$1")
    case "$(echo "$v" | tr 'A-Z' 'a-z')" in
        true|yes|1)  echo "true" ;;
        false|no|0)  echo "false" ;;
        *)           echo "$2" ;;
    esac
}

# Load the TUN kernel module and make sure /dev/net/tun exists.
#
# The firmware ships tun.ko but NEVER loads it — there is no init hook that
# does, so after every reboot /dev/net is missing entirely (the module
# creates the directory when it loads). Without this, transparent proxy
# cannot start and sockrocket-cli fails with "TUN device node /dev/net/tun is
# missing".
#
# Three levels, each verified by actually testing for the device rather than
# trusting an exit code — a modprobe that reports success but leaves no node
# is a real failure mode here:
#   1. modprobe   — resolves dependencies via /lib/modules/$(uname -r)/modules.dep
#   2. insmod     — path built from `uname -r`, NOT hardcoded, so a firmware
#                   upgrade that changes the kernel version still works
#   3. mknod      — module already loaded but devfs did not create the node
# Returns 0 only when the device is really present.
ensure_tun() {
    [ -e "$TUN_DEV" ] && return 0

    if which modprobe >/dev/null 2>&1; then
        modprobe tun 2>/dev/null || true
    fi

    if [ ! -e "$TUN_DEV" ] && which insmod >/dev/null 2>&1; then
        insmod "/lib/modules/$(uname -r)/kernel/drivers/net/tun.ko" 2>/dev/null || true
    fi

    if [ ! -e "$TUN_DEV" ]; then
        mkdir -p "$(dirname "$TUN_DEV")" 2>/dev/null || true
        mknod "$TUN_DEV" c 10 200 2>/dev/null || true
        chmod 600 "$TUN_DEV" 2>/dev/null || true
    fi

    [ -e "$TUN_DEV" ]
}

# Wait for Sockrocket's TUN device to appear (up to ~$1 seconds).
wait_for_tun() {
    local tries="${1:-20}"
    while [ "$tries" -gt 0 ]; do
        ip link show sockrocket-tun >/dev/null 2>&1 && return 0
        # Older builds used the kernel-assigned tun0.
        ip -4 addr show tun0 2>/dev/null | grep -q "10.10.0.2" && return 0
        # Stop early if the process died while we waited.
        is_running || return 1
        sleep 1
        tries=$((tries - 1))
    done
    return 1
}

# Poll until Sockrocket's DNS listener is bound. sockrocket-cli binds the DNS port
# asynchronously after startup; update_dnsmasq's single-shot listener check
# races it (observed on a cold start: the hijack was skipped even though the
# port came up ~2s later, leaving dns_hijack=true with no hijack until the
# next restart).
wait_for_dns() {
    local tries="${1:-15}" dns_port
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    while [ "$tries" -gt 0 ]; do
        netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)" && return 0
        # Stop early if the process died while we waited.
        is_running || return 1
        sleep 1
        tries=$((tries - 1))
    done
    return 1
}

# ── Stop ──────────────────────────────────────────────────────────────────────
# Fail-open: after stop/uninstall, LAN must work without any Sockrocket path
# (no MARK→TUN, no DNS hijack, no pin to 127.0.0.1#5300). Idempotent.
restore_direct_network() {
    # Transparent proxy + DNS NAT (script may already be gone on uninstall)
    if [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ]; then
        "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1 || true
        "$SOCKROCKET_DIR/scripts/iptables.sh" dns-stop >> "$SOCKROCKET_LOG" 2>&1 || true
    else
        iptables -t mangle -D PREROUTING -j SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -t mangle -F SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -t mangle -X SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -t nat -D PREROUTING -j SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -F SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -X SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -D PREROUTING -j SOCKROCKET_PREROUTING 2>/dev/null || true
        iptables -t nat -F SOCKROCKET_PREROUTING 2>/dev/null || true
        iptables -t nat -X SOCKROCKET_PREROUTING 2>/dev/null || true
        iptables -D FORWARD -i sockrocket-tun -j ACCEPT 2>/dev/null || true
        ip rule  del fwmark 0x4765 table 5370 2>/dev/null || true
        ip route flush table 5370 2>/dev/null || true
        ip rule  del fwmark 0x4765 table 100 2>/dev/null || true
    fi

    # dnsmasq hijack file + upstream pin
    update_dnsmasq stop

    # Belt: if pin survived, rebuild ISP upstreams from backup or /etc/resolv.conf
    local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
    if grep -q "^server=127.0.0.1#" "$resolv_file" 2>/dev/null; then
        log_warn "DNS pin still present after cleanup — force-restoring ISP upstreams"
        local bak_file="${resolv_file}.Sockrocket-bak"
        if [ -s "$bak_file" ]; then
            cp "$bak_file" "$resolv_file"
            rm -f "$bak_file"
        else
            awk '/^nameserver/ && $2 != "127.0.0.1" && $2 != "::1" {print "server=" $2}' \
                "${SOCKROCKET_RESOLV_CONF:-/etc/resolv.conf}" 2>/dev/null > "${resolv_file}.new" \
                && [ -s "${resolv_file}.new" ] && mv "${resolv_file}.new" "$resolv_file" \
                || rm -f "${resolv_file}.new"
        fi
        kill -HUP "$(pidof dnsmasq)" 2>/dev/null || true
    fi

    # Drop TUN device left by --tun so no stale routes linger
    ip link del sockrocket-tun 2>/dev/null || true
}

do_stop() {
    # Tear down BEFORE the early return: a crashed daemon (dead PID in
    # sockrocket.pid) used to skip all cleanup, leaving the iptables MARK rules and
    # the dnsmasq hijack (catch-all → 127.0.0.1:5300) live with nothing
    # listening — a LAN-wide DNS blackhole the Web UI could not clear.
    if is_running; then
        info "Stopping Sockrocket (API bridge keeps running so the Web UI can start it again)..."
        local pid
        pid=$(cat "$SOCKROCKET_PID")
        kill "$pid" 2>/dev/null
        local wait=0
        while kill -0 "$pid" 2>/dev/null && [ "$wait" -lt 8 ]; do
            sleep 1; wait=$((wait + 1))
        done
        kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    else
        info "Sockrocket process not running — cleaning up leftover rules anyway"
    fi
    rm -f "$SOCKROCKET_PID"

    kill_stray_daemons

    restore_direct_network
    remove_cron
    restore_core_pattern
    prune_core_dumps

    # The config keys express user INTENT and are deliberately left alone by
    # stop: restarting the service must restore the user's chosen toggles.
    : > "$SOCKROCKET_DIR/stopped"

    ok "Sockrocket stopped (direct LAN network restored)"
}

# ── Independent toggles ───────────────────────────────────────────────────────
# dns-on: install the dnsmasq split-DNS hijack. Refuses when Sockrocket's DNS port is
# not listening — the catch-all upstream would blackhole every LAN lookup.
do_dns_on() {
    if update_dnsmasq start; then
        [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ] && return 0
    fi
    return 1
}

do_dns_off() {
    update_dnsmasq stop
    [ ! -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]
}

# proxy-off: tear down ONLY the transparent-proxy rules. The daemon keeps
# running so SOCKS/HTTP stay reachable.
#
# This is a low-level primitive and is NOT durable on its own: the config key
# still says transparent_proxy=true, so the next watchdog pass would reapply
# the rules. Callers must write the key to false FIRST (cgi.rs's set_toggles
# does exactly that), after which the watchdog's key guard keeps it off.
# dns-off needs no such pairing — the DNS watchdog can only remove a hijack.
do_proxy_off() {
    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
    local rc=0
    iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK" && rc=1
    # A daemon started with --tun keeps its TUN stack up even after the
    # steering rules are gone; restart it in plain mode so the running
    # process matches the (now off) configuration.
    local pid
    pid=$(cat "$SOCKROCKET_PID" 2>/dev/null)
    if [ -n "$pid" ] \
        && tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q -- "--tun"; then
        do_stop
        sleep 1
        do_start
    fi
    return $rc
}

# ── Status ────────────────────────────────────────────────────────────────────
do_status() {
    if is_running; then
        local pid socks_port http_port active_node mode
        pid=$(cat "$SOCKROCKET_PID")
        socks_port=$(get_config_value socks_port)
        http_port=$(get_config_value http_port)
        active_node=$(get_config_value active_node)
        mode=$(get_config_value mode)
        ok "Sockrocket is running (PID $pid)"
        echo "  Mode   : ${mode:-tun}"
        echo "  SOCKS5 : 0.0.0.0:${socks_port:-1080}"
        echo "  HTTP   : 0.0.0.0:${http_port:-1087}"
        echo "  Node   : ${active_node:-0}"
        # DNS status
        [ -f "/jffs/configs/dnsmasq.d/sockrocket.conf" ] \
            && echo "  DNS    : active" || echo "  DNS    : inactive"
        # Firewall status
        iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q MARK \
            && echo "  FW     : active" || echo "  FW     : inactive"
    else
        info "Sockrocket is stopped"
    fi
    # API bridge state is reported in both cases (it must survive stop)
    api_is_running && echo "  API    : port $(api_port) (up)" \
                   || echo "  API    : down — Web UI unreachable (run: $0 api-start)"
}

# ── Update subscriptions ──────────────────────────────────────────────────────
do_update_subs() {
    info "Updating subscriptions (restart to re-fetch)..."
    if is_running; then
        do_stop
        sleep 1
        do_start
    else
        do_start
    fi
    ok "Subscription update triggered"
}

# ── Diagnose ──────────────────────────────────────────────────────────────────
cmd_diagnose() {
    echo "=== Sockrocket Diagnostic Report ==="
    echo "Date: $(date '+%Y-%m-%d %H:%M:%S')"
    echo ""
    echo "--- Service Status ---"
    if is_running; then
        local pid mode
        pid=$(cat "$SOCKROCKET_PID")
        mode=$(get_config_value mode)
        echo "  sockrocket-cli : RUNNING (pid=$pid, mode=${mode:-tun})"
        # Uptime
        if [ -f "/proc/$pid/stat" ]; then
            local start_ticks btime now uptime
            start_ticks=$(awk '{print $22}' "/proc/$pid/stat" 2>/dev/null || echo 0)
            btime=$(awk '/^btime/{print $2}' /proc/stat 2>/dev/null || echo 0)
            now=$(date +%s)
            if [ "$start_ticks" -gt 0 ] && [ "$btime" -gt 0 ]; then
                local start_sec=$(( btime + start_ticks / 100 ))
                uptime=$(( now - start_sec ))
                echo "  Uptime   : $(( uptime / 3600 ))h $(( (uptime%3600)/60 ))m $(( uptime%60 ))s"
            fi
        fi
    else
        echo "  sockrocket-cli : STOPPED"
        [ -f "$SOCKROCKET_PID" ] && echo "  (stale PID file exists: $(cat "$SOCKROCKET_PID"))"
    fi

    echo ""
    echo "--- Proxy Ports ---"
    local socks_port http_port dns_port
    socks_port=$(get_config_value socks_port); socks_port=${socks_port:-1080}
    http_port=$(get_config_value http_port);   http_port=${http_port:-1087}
    dns_port=$(get_config_value dns_port);     dns_port=${dns_port:-5300}
    echo "  SOCKS5 : $socks_port"
    echo "  HTTP   : $http_port"
    echo "  DNS    : $dns_port"

    echo ""
    echo "--- iptables Rules ---"
    local count
    count=$(iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -cE "MARK")
    echo "  SOCKROCKET_MANGLE (mangle): ${count:-0} rules active"
    iptables -t mangle -C PREROUTING -j SOCKROCKET_MANGLE 2>/dev/null && echo "  Jump SOCKROCKET_MANGLE \342\206\222 PREROUTING : INSTALLED" || echo "  Jump SOCKROCKET_MANGLE \342\206\222 PREROUTING : MISSING"

    echo ""
    echo "--- DNS ---"
    local dns_running="NO"
    pgrep dnsmasq >/dev/null 2>&1 && dns_running="YES"
    echo "  dnsmasq running : $dns_running"
    [ -f "/jffs/configs/dnsmasq.d/sockrocket.conf" ] && echo "  dnsmasq sockrocket.conf : EXISTS" || echo "  dnsmasq sockrocket.conf : MISSING"

    echo ""
    echo "--- Config Validation ---"
    if [ -f "$SOCKROCKET_CONF" ]; then
        QUERY_STRING="action=diagnose" "$SOCKROCKET_BIN" cgi 2>/dev/null | grep -q '"config_valid":true' \
            && echo "  Config : VALID" || echo "  Config : INVALID (check logs)"
    else
        echo "  Config : MISSING"
    fi

    echo ""
    echo "--- Binary ---"
    if [ -x "$SOCKROCKET_BIN" ]; then
        echo "  Binary : $("$SOCKROCKET_BIN" --version 2>/dev/null | head -1 || echo 'READ FAILED')"
    else
        echo "  Binary : MISSING or NOT EXECUTABLE"
    fi

    echo ""
    echo "--- Disk ---"
    df -h /jffs 2>/dev/null | tail -1
    echo ""
    echo "--- Last 10 Errors ---"
    grep -i "error\|fail\|panic" "$SOCKROCKET_LOG" 2>/dev/null | tail -10 || echo "  (no errors in log)"
    echo ""
    echo "=== End Report ==="
}

# ── Crash forensics ("black box") ─────────────────────────────────────────────
# Text history is always on (bounded crash-history.log). Core dumps are opt-in
# via SOCKROCKET_CORE_DUMPS=1: writing a system-wide core_pattern and
# `ulimit -c unlimited` on a 256–512 MB router can OOM the box when anything
# segfaults. When enabled we snapshot the previous pattern and restore it on
# stop, and keep at most CORE_KEEP_MAX gzipped cores on /tmp.
enable_core_dumps() {
    # Always prune leftover cores from a previous opt-in session so /tmp
    # cannot accumulate them across reboots of the service.
    prune_core_dumps
    [ "$SOCKROCKET_CORE_DUMPS" = "1" ] || return 0
    # Cap core size (~16 MiB with 512-byte busybox blocks; ~16 MiB with 1 KiB
    # bash blocks). Enough for a useful backtrace, small enough for tmpfs.
    ulimit -c 32768 2>/dev/null || true
    if [ -w /proc/sys/kernel/core_pattern ]; then
        # Remember the firmware pattern once so stop can restore it.
        if [ ! -f "$SOCKROCKET_CORE_PATTERN_BAK" ]; then
            cat /proc/sys/kernel/core_pattern > "$SOCKROCKET_CORE_PATTERN_BAK" 2>/dev/null || true
        fi
        echo "$SOCKROCKET_CORE_PATTERN" > /proc/sys/kernel/core_pattern 2>/dev/null || true
    fi
}

restore_core_pattern() {
    [ -f "$SOCKROCKET_CORE_PATTERN_BAK" ] || return 0
    if [ -w /proc/sys/kernel/core_pattern ]; then
        cat "$SOCKROCKET_CORE_PATTERN_BAK" > /proc/sys/kernel/core_pattern 2>/dev/null || true
    fi
    rm -f "$SOCKROCKET_CORE_PATTERN_BAK"
}

# Keep at most CORE_KEEP_MAX newest sockrocket-core*.gz; delete raw cores after
# gzip. Safe to call when dumps are disabled (clears leftovers).
prune_core_dumps() {
    local core n
    for core in "$SOCKROCKET_CORE_DIR"/sockrocket-core.*; do
        [ -f "$core" ] || continue
        case "$core" in
            *.gz) ;;
            *) gzip -9 "$core" 2>/dev/null || rm -f "$core" ;;
        esac
    done
    # Newest first; drop extras. `ls -t` is available on busybox.
    n=0
    for core in $(ls -t "$SOCKROCKET_CORE_DIR"/sockrocket-core.*.gz 2>/dev/null); do
        n=$((n + 1))
        [ "$n" -gt "$CORE_KEEP_MAX" ] && rm -f "$core"
    done
}

collect_crash_dump() {
    local when pid_note
    when=$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || echo unknown)
    pid_note=$(cat "$SOCKROCKET_PID" 2>/dev/null || echo '?')
    {
        echo "=== crash detected: $when (pidfile=$pid_note) ==="
        echo "--- dmesg tail ---"
        dmesg 2>/dev/null | tail -30
        echo "--- sockrocket.log tail ---"
        tail -50 "$SOCKROCKET_LOG" 2>/dev/null
        echo "--- uptime / memory ---"
        uptime 2>/dev/null; free 2>/dev/null
        echo ""
    } >> "$SOCKROCKET_CRASH_LOG" 2>/dev/null
    if [ "$SOCKROCKET_CORE_DUMPS" = "1" ]; then
        local core
        for core in "$SOCKROCKET_CORE_DIR"/sockrocket-core.*; do
            [ -f "$core" ] || continue
            case "$core" in *.gz) continue ;; esac
            gzip -9 "$core" 2>/dev/null && \
                echo "core preserved: ${core}.gz ($(ls -l "${core}.gz" 2>/dev/null | awk '{print $5}') bytes)" >> "$SOCKROCKET_CRASH_LOG"
        done
        prune_core_dumps
    fi
    # Keep the text history bounded: crash sections are ~4 KB, 200 KB holds
    # roughly the last 50 crashes; jffs wear stays negligible at that rate.
    local size
    size=$(ls -l "$SOCKROCKET_CRASH_LOG" 2>/dev/null | awk '{print $5}')
    if [ -n "$size" ] && [ "$size" -gt "$CRASH_LOG_MAX_BYTES" ]; then
        tail -c $((CRASH_LOG_MAX_BYTES / 2)) "$SOCKROCKET_CRASH_LOG" > "$SOCKROCKET_CRASH_LOG.tmp" 2>/dev/null && \
            mv "$SOCKROCKET_CRASH_LOG.tmp" "$SOCKROCKET_CRASH_LOG"
    fi
}

# ── Watchdog ──────────────────────────────────────────────────────────────────
do_watchdog() {
    local issues=0

    # Bound the jffs log even when the daemon stays healthy for weeks —
    # otherwise verbose tracing fills flash between restarts.
    rotate_log
    prune_core_dumps

    # User intent: after a manual stop (marker written by do_stop) the
    # watchdog must leave the service alone. Only unexpected crashes get
    # auto-recovery. The API bridge is still kept up so the Web UI can
    # start the service again.
    if [ -f "$SOCKROCKET_DIR/stopped" ]; then
        if ! api_is_running; then
            do_api_start >> "$SOCKROCKET_LOG" 2>&1
        fi
        return
    fi

    local healthy=1
    if ! is_running; then
        log_error "Watchdog: Sockrocket not running — restarting..."
        # Grab the crash scene BEFORE the restart wipes it: dmesg registers,
        # log tail, and any core the kernel wrote to /tmp. The restart itself
        # clears /tmp? no — tmpfs persists until reboot — but a second crash
        # would overwrite the core, and dmesg scrolls, so collect now.
        collect_crash_dump
        do_start >> "$SOCKROCKET_LOG" 2>&1
        issues=$((issues + 1))
        healthy=0
        # The restart can itself fail (e.g. tun.ko not loaded), so the
        # fail-open check at the end still has to run — a hijack left over
        # from the previous run must not stay installed against a dead port.
    fi

    if [ "$healthy" = "1" ]; then
        # The Web UI API bridge must stay up even while the proxy is stopped
        if ! api_is_running; then
            log_warn "Watchdog: API bridge down — restarting..."
            do_api_start >> "$SOCKROCKET_LOG" 2>&1
            issues=$((issues + 1))
        fi

        # Check proxy port reachable. Busybox builds without the `timeout`
        # applet would make this probe fail permanently → false "unreachable"
        # warnings every 5 minutes; fall back to plain nc in that case.
        local http_port port port_ok=1
        http_port=$(get_config_value http_port)
        port=${http_port:-1087}
        if which timeout >/dev/null 2>&1; then
            timeout 3 sh -c "echo | nc -w1 127.0.0.1 $port >/dev/null 2>&1" || port_ok=0
        else
            nc -w1 127.0.0.1 "$port" </dev/null >/dev/null 2>&1 || port_ok=0
        fi
        if [ "$port_ok" = "0" ]; then
            log_warn "Watchdog: HTTP proxy port $port unreachable"
            issues=$((issues + 1))
        fi

        # Self-heal a rebooted router: tun.ko never auto-loads, so if the
        # user wants transparent proxying and the device is gone, reload it
        # before reapplying the rules.
        if [ "$(conf_bool transparent_proxy true)" = "true" ]; then
            ensure_tun || log_warn "Watchdog: TUN unavailable for transparent proxy"
            if { ip link show sockrocket-tun >/dev/null 2>&1 || \
                 ip -4 addr show tun0 2>/dev/null | grep -q "10.10.0.2"; } \
                && ! iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                log_warn "Watchdog: iptables rules missing, reapplying..."
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" start >> "$SOCKROCKET_LOG" 2>&1
                issues=$((issues + 1))
            fi
        else
            # Symmetry with the DNS side: when the proxy is switched off a
            # stale MARK chain must not survive. Traffic marked into a TUN
            # device that may no longer exist blackholes the LAN.
            if iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                log_warn "Watchdog: transparent proxy is off but stale rules exist — removing"
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
                issues=$((issues + 1))
            fi
        fi
    fi

    # DNS hijack policy (ordered by user intent, fail-open where needed):
    #   - zero nodes in config  → never leave hijack/TUN up (Softcenter + LAN
    #     DNS would hang even if toggles say true).
    #   - dns_hijack disabled   → sockrocket.conf must not exist at all (stale
    #     leftovers break LAN DNS).
    #   - daemon confirmed dead → fail open: a hijack pinned to a dead
    #     127.0.0.1:<port> would blackhole LAN DNS.
    #   - daemon alive          → the hijack SHOULD exist, so SELF-HEAL a
    #     missing sockrocket.conf and re-pin the upstream; never tear down on a
    #     single bad port reading. A lone busybox `netstat` snapshot has
    #     produced false negatives (output-format quirks, races with a
    #     daemon restart) and one such reading used to remove the whole
    #     hijack — observed as "sockrocket.conf vanished while the
    #     daemon was healthy". The probe therefore double-checks within
    #     one run and only fails open after 3 consecutive failing runs.
    local dns_port dns_conf="${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf"
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    local want_dns
    want_dns=$(conf_bool dns_hijack true)
    local fail_file="$SOCKROCKET_DIR/.wdns_fails"

    if [ "$(count_section nodes)" = "0" ]; then
        if [ -f "$dns_conf" ]; then
            log_error "Watchdog: no nodes in config — removing DNS hijack (LAN/Softcenter restored)"
            update_dnsmasq stop
            issues=$((issues + 1))
        fi
        if iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
            log_error "Watchdog: no nodes in config — removing transparent proxy rules"
            [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
            issues=$((issues + 1))
        fi
        rm -f "$fail_file"
    elif [ "$want_dns" = "false" ]; then
        if [ -f "$dns_conf" ]; then
            log_error "Watchdog: dns_hijack disabled but hijack still installed — removing it (LAN DNS restored)"
            update_dnsmasq stop
            issues=$((issues + 1))
        fi
        rm -f "$fail_file"
    elif ! is_running; then
        # The restart attempt above already ran; still-dead means fail open.
        if [ -f "$dns_conf" ]; then
            log_error "Watchdog: Sockrocket daemon dead — removing DNS hijack (LAN DNS restored)"
            update_dnsmasq stop
            issues=$((issues + 1))
        fi
        rm -f "$fail_file"
    else
        local port_ok=0 i
        for i in 1 2; do
            if netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)"; then
                port_ok=1
                break
            fi
            [ "$i" = "1" ] && sleep 1
        done

        if [ "$port_ok" = "1" ]; then
            rm -f "$fail_file"
            if [ ! -f "$dns_conf" ]; then
                log_warn "Watchdog: DNS hijack missing while daemon is up — reinstalling"
                update_dnsmasq start
                issues=$((issues + 1))
            elif ! iptables -t nat -C PREROUTING -j SOCKROCKET_DNS 2>/dev/null; then
                # firewall-start used to flush DNS NAT; heal it independently of
                # sockrocket.conf so hard-coded 8.8.8.8 clients stay redirected.
                log_warn "Watchdog: DNS NAT chain missing — reapplying dns-start"
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                    "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start >> "$SOCKROCKET_LOG" 2>&1
                issues=$((issues + 1))
            fi
            # WAN reconfiguration / dhcp renew regenerates resolv.dnsmasq
            # with ISP servers, silently un-pinning the upstream. Re-pin.
            local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
            local before
            before=$(cat "$resolv_file" 2>/dev/null)
            pin_dns_upstream
            [ "$(cat "$resolv_file" 2>/dev/null)" != "$before" ] && \
                log_warn "Watchdog: dnsmasq upstream was unpinned (ISP DNS had returned) — re-pinned to Sockrocket"
        else
            local fails=0
            [ -f "$fail_file" ] && fails=$(cat "$fail_file" 2>/dev/null)
            fails=$((fails + 1))
            echo "$fails" > "$fail_file"
            if [ "$fails" -ge 3 ] && [ -f "$dns_conf" ]; then
                log_error "Watchdog: DNS port $dns_port dead for $fails consecutive checks — failing open (LAN DNS restored)"
                update_dnsmasq stop
                rm -f "$fail_file"
                issues=$((issues + 1))
            else
                log_warn "Watchdog: DNS port $dns_port not listening (consecutive failure $fails/3) — keeping current DNS state"
            fi
        fi
    fi

    [ "$issues" -gt 0 ] && log_warn "Watchdog: $issues issue(s) found and addressed"
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
# Ensure lock is released on script exit (even on signal).
# INT/TERM must exit after the handler — the EXIT trap then releases the
# lock. Trapping them without exiting would let a killed start/stop run on.
trap 'release_lock' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

case "$1" in
    start)
        acquire_lock && { do_start; release_lock; } || true
        ;;
    stop)
        acquire_lock && { do_stop; release_lock; } || true
        ;;
    restart)
        acquire_lock && { do_stop; sleep 1; do_start; release_lock; } || true
        ;;
    reload)
        acquire_lock || exit 1
        if is_running; then
            info "Reloading Sockrocket config..."
            do_stop; sleep 1; do_start
        else
            do_start
        fi
        release_lock
        ;;
    status)  do_status ;;
    log)     tail -100 "$SOCKROCKET_LOG" 2>/dev/null || info "No log file found" ;;
    api-start)
        acquire_lock 5 && { do_api_start; release_lock; } || true
        ;;
    api-stop)
        acquire_lock 5 && { do_api_stop; release_lock; } || true
        ;;
    api-restart)
        acquire_lock 5 && { do_api_stop; do_api_start; release_lock; } || true
        ;;
    update-subs)
        acquire_lock && { do_update_subs; release_lock; } || true
        ;;
    watchdog) acquire_lock 5 && { do_watchdog; release_lock; } || log_warn "Watchdog: previous run still active, skipping" ;;
    ensure-tun)
        acquire_lock 5 && { ensure_tun && log_info "TUN device ready" || log_error "TUN unavailable"; release_lock; } || true
        ;;
    dns-on)
        acquire_lock && { do_dns_on && log_ok "DNS hijack enabled" || log_error "Failed to enable DNS hijack"; release_lock; } || true
        ;;
    dns-off)
        acquire_lock && { do_dns_off && log_ok "DNS hijack disabled" || log_error "Failed to disable DNS hijack"; release_lock; } || true
        ;;
    pin-dns)
        acquire_lock 5 && { pin_dns_upstream; release_lock; } || true
        ;;
    proxy-off)
        acquire_lock && { do_proxy_off && log_ok "Transparent proxy disabled" || log_error "Failed to disable transparent proxy"; release_lock; } || true
        ;;
    network-restore)
        # Emergency / uninstall helper: tear down proxy+DNS without needing a
        # running daemon. Guarantees direct LAN connectivity.
        acquire_lock && { restore_direct_network; ok "Direct network restored"; release_lock; } || true
        ;;
    diagnose)  cmd_diagnose ;;
    *)
        echo "Usage: $0 {start|stop|restart|reload|status|log|diagnose|update-subs|watchdog|api-start|api-stop|api-restart|ensure-tun|dns-on|dns-off|pin-dns|proxy-off|network-restore}"
        exit 1
        ;;
esac
