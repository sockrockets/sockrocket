#!/bin/sh
# Sockrocket proxy service control script for AsusWRT-Merlin
#
# Usage: sockrocket.sh {boot|start|stop|restart|reload|status|software-center-status|log|diagnose|update-subs|watchdog|api-start|api-stop|api-restart}

# Configuration, logs and crash artifacts are private by default. Firmware
# dnsmasq inputs explicitly opt into 0644 at their write sites.
umask 077

SOCKROCKET_DIR="/jffs/addons/sockrocket"
SOCKROCKET_BIN="$SOCKROCKET_DIR/sockrocket-cli"
SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
SOCKROCKET_SCRIPTS="$SOCKROCKET_DIR/scripts"
SOCKROCKET_PID="$SOCKROCKET_DIR/sockrocket.pid"
SOCKROCKET_LOG="$SOCKROCKET_DIR/sockrocket.log"
SOCKROCKET_LOCK="$SOCKROCKET_DIR/sockrocket.lock"
# Runtime fail-open marker: present ⇒ LAN uses direct DNS/routing; config
# toggles still express user intent and are re-applied when the probe recovers.
DEGRADED_FILE="$SOCKROCKET_DIR/.lan_degraded"
WOUT_FAILS_FILE="$SOCKROCKET_DIR/.wout_fails"
# Epoch seconds before which fail-open recovery must not restart dnsmasq.
# A missed isolation check used to retry every few seconds and keep LAN DNS down.
RECOVER_AFTER_FILE="$SOCKROCKET_DIR/.recover_after"
SUPERVISOR_PID="$SOCKROCKET_DIR/sockrocket-supervisor.pid"
GUARD_PID_FILE="$SOCKROCKET_DIR/sockrocket-guard.pid"
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
# Keep the token-protected loopback API alive while the proxy is stopped.
# Browser requests use firmware CGI or Software Center RPC; the browser cannot
# reach this API directly from the LAN.
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

# Software Center's installed bit is not its enabled bit. Only the proxy
# daemon counts; the always-on API and a saved configuration do not.
sync_softcenter_state() {
    which dbus >/dev/null 2>&1 || return 0
    local enabled=0 key
    if [ ! -f "$SOCKROCKET_DIR/stopped" ] && is_running; then enabled=1; fi
    for key in sockrocket_enable softcenter_module_sockrocket_enable; do
        # Avoid repeated persistent writes from watchdog/status polling.
        [ "$(dbus get "$key" 2>/dev/null)" = "$enabled" ] || \
            dbus set "$key=$enabled" >/dev/null 2>&1 || true
    done
}

do_softcenter_status() {
    sync_softcenter_state
    if [ ! -f "$SOCKROCKET_DIR/stopped" ] && is_running; then
        printf 'Sockrocket running (PID %s)\n' "$(cat "$SOCKROCKET_PID")"
    else
        # Some software centers detect running by the literal "PID". Never
        # include the independent API's PID in this response.
        printf 'Sockrocket stopped\n'
    fi
}

do_api_start() {
    sync_softcenter_state
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

run_locked() {
    local timeout="$1" result
    shift
    acquire_lock "$timeout" || return 1
    "$@"
    result=$?
    release_lock
    return "$result"
}

do_restart() {
    # Background subscription/config writes must not revoke a manual Stop.
    # The explicit WebUI Restart action uses start when already stopped.
    if [ -f "$SOCKROCKET_DIR/stopped" ]; then
        sync_softcenter_state
        info "Service manually stopped; saved settings will apply on next start"
        return 0
    fi
    do_stop && do_start
}

# Installations made before nat-start existed only regain DNS hijack on the
# next watchdog minute. Register the hook the first time this script runs.
ensure_nat_hook() {
    local script="/jffs/scripts/nat-start" target
    [ -d /jffs/scripts ] || return 0
    target="$SOCKROCKET_DIR/scripts/sockrocket.sh"
    if [ -f "$script" ] && grep -q 'nat-restore' "$script" 2>/dev/null; then
        return 0
    fi
    if [ ! -f "$script" ]; then
        printf '#!/bin/sh\n' > "$script" || return 0
        chmod +x "$script" 2>/dev/null || true
    fi
    {
        echo "# >>> SOCKROCKET_AUTO_START >>>"
        echo "# Auto-generated by Sockrocket. Do not edit between markers."
        printf '[ -x %s ] && %s nat-restore\n' "$target" "$target"
        echo "# <<< SOCKROCKET_AUTO_END <<<"
    } >> "$script" || return 0
    chmod +x "$script" 2>/dev/null || true
}

do_boot() {
    ensure_nat_hook
    do_api_start || log_warn "Management API unavailable during boot; preserving proxy intent"
    if [ -f "$SOCKROCKET_DIR/stopped" ]; then
        sync_softcenter_state
        return 0
    fi
    do_start
}

# ── Log rotation ──────────────────────────────────────────────────────────────
rotate_log() {
    [ ! -f "$SOCKROCKET_CRASH_LOG" ] || chmod 600 "$SOCKROCKET_CRASH_LOG" || return 1
    [ -f "$SOCKROCKET_LOG" ] || return 0
    chmod 600 "$SOCKROCKET_LOG" || return 1
    local size
    size=$(wc -c < "$SOCKROCKET_LOG" 2>/dev/null || echo 0)
    if [ "$size" -gt "$LOG_MAX_BYTES" ]; then
        # Keep the inode: daemon/API writers hold O_APPEND descriptors open.
        # Renaming a replacement leaves them writing an unlinked, unbounded
        # file on JFFS until the next restart.
        if tail -c "$((LOG_MAX_BYTES / 2))" "$SOCKROCKET_LOG" > "$SOCKROCKET_LOG.tmp"; then
            cat "$SOCKROCKET_LOG.tmp" > "$SOCKROCKET_LOG"
        fi
        rm -f "$SOCKROCKET_LOG.tmp"
        info "Log rotated (was ${size} bytes)"
    fi
}

# ── Cron integration ──────────────────────────────────────────────────────────
setup_cron() {
    # Subscription auto-update daily at 04:00
    cru a SockrocketSubUpdate "0 4 * * * $SOCKROCKET_DIR/scripts/sockrocket.sh update-subs" 2>/dev/null || true
    # Slow safety net only — second-scale recovery is the in-process
    # supervisor + guard loop started by do_start.
    cru a SockrocketWatchdog "*/1 * * * * $SOCKROCKET_DIR/scripts/sockrocket.sh watchdog" 2>/dev/null || true
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
#
# Fail-open defaults: missing keys must NOT enable DNS hijack. A missing
# dns_hijack on an old config used to become true whenever mode=tun, which
# blackholed Softcenter/LAN DNS on first boot before the user opted in.
migrate_toggle_keys() {
    [ -f "$SOCKROCKET_CONF" ] || return 0
    [ -n "$(get_config_value transparent_proxy)" ] && [ -n "$(get_config_value dns_hijack)" ] && return 0
    local mode tp dh
    mode=$(get_config_value mode)
    case "${mode:-tun}" in
        tun) tp=true;  dh=false ;;
        *)   tp=false; dh=false ;;
    esac
    local tmp="$SOCKROCKET_CONF.tmp"
    {
        grep -q "^transparent_proxy:" "$SOCKROCKET_CONF" || echo "transparent_proxy: $tp"
        grep -q "^dns_hijack:"        "$SOCKROCKET_CONF" || echo "dns_hijack: $dh"
        cat "$SOCKROCKET_CONF"
    } > "$tmp" && mv "$tmp" "$SOCKROCKET_CONF"
}

# Pin the main dnsmasq upstream to Sockrocket's DNS listener.
#
# Merlin 3006 SDNs can share /tmp/resolv.dnsmasq with the main LAN. On 3006
# the private file is only the activation marker and the upstream line.
# dnsmasq.postconf removes servers-file and resolv-file from the main config
# and inserts server=127.0.0.1#port, which is how fancyss isolates that
# instance. Older firmware keeps the shared-file pin and backup path below.
pin_dns_upstream() {
    local dns_port dns_conf="${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf"
    local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
    local bak_file="${resolv_file}.Sockrocket-bak"
    local private_file="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
    # A leftover dnsmasq snippet alone is not permission to point LAN DNS at
    # this process. Degraded recovery deliberately calls this before clearing
    # its fail-open marker, so only the event entry point checks that marker.
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    [ "$(conf_bool dns_hijack false)" = "true" ] || return 0
    is_running || return 0
    [ -f "$dns_conf" ] || return 0
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    # Only pin while our listener is actually answering.
    netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)" || return 0
    if merlin_3006; then
        # SDN instances can read the firmware's shared resolv.dnsmasq. The
        # main postconf hook reads this file and does not point them at it.
        local hook="${SOCKROCKET_DNS_POSTCONF_HOOK:-/jffs/scripts/dnsmasq.postconf}"
        local helper="${SOCKROCKET_DNS_POSTCONF_SCRIPT:-$SOCKROCKET_DIR/scripts/dnsmasq-postconf.sh}"
        local main_pid
        [ -x "$helper" ] && grep -q 'dnsmasq-postconf.sh' "$hook" 2>/dev/null || {
            log_error "Merlin 3006 DNS postconf hook missing; DNS interception disabled"
            return 1
        }
        if [ "$(cat "$private_file" 2>/dev/null)" = "server=127.0.0.1#${dns_port}" ]; then
            # dnsmasq runs as nobody. A 0600 file is an empty upstream list
            # until the main instance is told to read it again.
            case "$(ls -l "$private_file" 2>/dev/null)" in
                -rw-r--r--*) return 0 ;;
            esac
            chmod 644 "$private_file" || return 1
            main_pid=$(cat "${SOCKROCKET_DNS_MAIN_PID:-/var/run/dnsmasq.pid}" 2>/dev/null)
            case "$main_pid" in ''|*[!0-9]*) ;; *) kill -HUP "$main_pid" 2>/dev/null || true ;; esac
            return 0
        fi
        # Merlin can ship BusyBox without a mktemp applet in PATH. An
        # exclusive, private directory also keeps the temporary file safe
        # from symlinks while allowing an atomic rename on the same filesystem.
        local private_tmp="${private_file}.$$"
        (umask 077; mkdir "$private_tmp") || return 1
        printf 'server=127.0.0.1#%s\n' "$dns_port" > "$private_tmp/upstream" && \
            chmod 644 "$private_tmp/upstream" && mv -f "$private_tmp/upstream" "$private_file" || {
            rm -f "$private_tmp/upstream"
            rmdir "$private_tmp" 2>/dev/null || true
            return 1
        }
        rmdir "$private_tmp" 2>/dev/null || true
        main_pid=$(cat "${SOCKROCKET_DNS_MAIN_PID:-/var/run/dnsmasq.pid}" 2>/dev/null)
        case "$main_pid" in ''|*[!0-9]*) ;; *) kill -HUP "$main_pid" 2>/dev/null || true ;; esac
        # The main config is not isolated yet. restart_dnsmasq still has to
        # run the postconf hook; this line only means the marker file exists.
        ok "Merlin 3006 private DNS upstream file ready"
        return 0
    fi
    # Skip rewriting a matching pin, but heal permissions from older builds.
    if [ "$(cat "$resolv_file" 2>/dev/null)" = "server=127.0.0.1#${dns_port}" ]; then
        # Older versions wrote 0600; matching contents do not imply readable
        # DNS configuration after dnsmasq drops root privileges.
        case "$(ls -l "$resolv_file" 2>/dev/null)" in
            -rw-r--r--*) return 0 ;;
        esac
        chmod 644 "$resolv_file" || return 1
        # dnsmasq may already have failed its read; permission repair alone
        # does not reload the upstream list.
        kill -HUP "$(pidof dnsmasq)" 2>/dev/null || true
        return 0
    fi
    # Back up the firmware's ISP upstreams BEFORE the first overwrite — the
    # stop path restores them (see unpin_dns_upstream). Never snapshot our own
    # pin: a restart while pinned would "restore" a dead upstream forever.
    if ! grep -q "^server=127.0.0.1#" "$resolv_file" 2>/dev/null && [ -f "$resolv_file" ]; then
        (umask 077; cp "$resolv_file" "${bak_file}.new") && mv -f "${bak_file}.new" "$bak_file" || {
            rm -f "${bak_file}.new"
            log_error "Cannot back up DNS upstreams; leaving existing DNS unchanged"
            return 1
        }
    fi
    (umask 022; printf 'server=127.0.0.1#%s\n' "$dns_port" > "${resolv_file}.new") && \
        chmod 644 "${resolv_file}.new" && mv -f "${resolv_file}.new" "$resolv_file" || { rm -f "${resolv_file}.new"; return 1; }
    kill -HUP "$(pidof dnsmasq)" 2>/dev/null
    ok "dnsmasq upstream pinned to 127.0.0.1#${dns_port} (LAN queries no longer go through ISP DNS)"
}

merlin_3006() {
    case "$(nvram get firmver 2>/dev/null)" in 3.0.0.6*) return 0 ;; *) return 1 ;; esac
}

# The file the main dnsmasq process reads. Tests pin one path. On the router,
# /etc/dnsmasq.conf and /tmp/etc/dnsmasq.conf are usually the same file; when
# they differ, the main instance is the one with the main pid-file.
main_dnsmasq_config() {
    if [ -n "${SOCKROCKET_DNS_MAIN_CONFIG+x}" ]; then
        printf '%s\n' "$SOCKROCKET_DNS_MAIN_CONFIG"
        return
    fi
    local candidate
    for candidate in /etc/dnsmasq.conf /tmp/etc/dnsmasq.conf; do
        [ -f "$candidate" ] || continue
        if grep -q '^pid-file=/var/run/dnsmasq\.pid$' "$candidate" 2>/dev/null; then
            printf '%s\n' "$candidate"
            return
        fi
    done
    if [ -f /etc/dnsmasq.conf ]; then
        printf '%s\n' /etc/dnsmasq.conf
        return
    fi
    printf '%s\n' /tmp/etc/dnsmasq.conf
}

# True when the main config cannot fall back to the shared resolv or
# /etc/resolv.conf and names Sockrocket as an upstream. Guest configs are
# a different file and are not checked here.
main_dns_isolated() {
    local config="$1" dns_port="$2"
    [ -s "$config" ] || return 1
    grep -q 'servers-file' "$config" && return 1
    grep -q 'resolv-file' "$config" && return 1
    grep -qx 'no-resolv' "$config" || return 1
    grep -qx "server=127.0.0.1#${dns_port}" "$config"
}

# Flags only. The config also holds DHCP ranges and names, which stay out of the log.
main_dns_state() {
    local config="$1" dns_port="$2"
    local servers=no resolv=no nore=no upstream=no
    [ -s "$config" ] || { printf 'empty'; return; }
    grep -q 'servers-file' "$config" && servers=yes
    grep -q 'resolv-file' "$config" && resolv=yes
    grep -qx 'no-resolv' "$config" && nore=yes
    grep -qx "server=127.0.0.1#${dns_port}" "$config" && upstream=yes
    printf 'servers-file=%s resolv-file=%s no-resolv=%s sockrocket-upstream=%s' \
        "$servers" "$resolv" "$nore" "$upstream"
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
        (umask 022; cp "$bak_file" "${resolv_file}.new") && \
            chmod 644 "${resolv_file}.new" && mv -f "${resolv_file}.new" "$resolv_file" || {
            rm -f "${resolv_file}.new"
            log_error "DNS upstream restore failed; backup retained for retry"
            return 1
        }
        rm -f "$bak_file"
    else
        # No backup (pinned by an older build): rebuild from the system
        # resolver, skipping loopback entries that would loop dnsmasq into
        # itself. Skip the write entirely if nothing usable was found.
        local rebuilt
        rebuilt=$(awk '/^nameserver/ && $2 != "127.0.0.1" && $2 != "::1" {print "server=" $2}' \
            "${SOCKROCKET_RESOLV_CONF:-/etc/resolv.conf}" 2>/dev/null)
        [ -n "$rebuilt" ] || { log_error "No direct DNS upstream available; recovery required"; return 1; }
        (umask 022; printf '%s\n' "$rebuilt" > "${resolv_file}.new") && \
            chmod 644 "${resolv_file}.new" && mv -f "${resolv_file}.new" "$resolv_file" || { rm -f "${resolv_file}.new"; return 1; }
    fi
    kill -HUP "$(pidof dnsmasq)" 2>/dev/null
    ok "dnsmasq upstream restored to ISP DNS"
}

dns_listener_healthy() {
    local dns_port
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    # A real answer is authoritative; a transient socket-table read must
    # not classify a responding DNS service as dead.
    "$SOCKROCKET_BIN" --dns-health "$dns_port" >/dev/null 2>&1
}

update_dnsmasq() {
    local action="$1"
    # Overridable for tests/integration/sockrocket_sh_test.sh (default stays the firmware path).
    local DNSMASQ_CONF_DIR="${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}"
    local SOCKROCKET_DNSMASQ="$DNSMASQ_CONF_DIR/sockrocket.conf"
    local TEMPLATE="$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"
    local isolated=0 waited=0 wait_limit

    mkdir -p "$DNSMASQ_CONF_DIR"

    if [ "$action" = "start" ] && [ -f "$TEMPLATE" ]; then
        local dns_port router_ip
        dns_port=$(get_config_value dns_port)
        dns_port=${dns_port:-5300}
        # Fail-safe: only hijack default DNS when Sockrocket's DNS listener is
        # actually up. The template's catch-all upstream would otherwise
        # blackhole every LAN lookup, taking the whole network offline.
        # Matches ":port " (followed by the peer column) or ":port" at
        # end-of-line, since busybox netstat column spacing varies.
        if ! dns_listener_healthy; then
            update_dnsmasq stop
            warn "Sockrocket DNS port $dns_port not listening -- DNS split not enabled (LAN DNS unchanged)"
            return 1
        fi
        router_ip=$(nvram get lan_ipaddr 2>/dev/null)
        [ -n "$router_ip" ] || {
            log_error "Primary LAN address unavailable; DNS interception disabled"
            update_dnsmasq stop
            return 1
        }
        if merlin_3006 && grep -q '^server=127.0.0.1#' "${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}" 2>/dev/null; then
            # Upgrade from the old shared-file pin before a new SDN starts.
            unpin_dns_upstream || return 1
        fi
        sed -e "s/__DNS_PORT__/$dns_port/g" \
            -e "s/__ROUTER_IP__/$router_ip/g" \
            "$TEMPLATE" > "$SOCKROCKET_DNSMASQ"
        if merlin_3006 && ! pin_dns_upstream; then
            update_dnsmasq stop
            return 1
        fi
        # NOTE: the old dns_direct_domains bypass (dnsmasq server=/ entries
        # straight to the domestic resolver) was removed: it
        # short-circuited Sockrocket's resolver, so those names never entered the
        # domestic-IP table (geoip misses then misrouted their bare-IP
        # traffic into the proxy — a domestic-site misroute) and user routing
        # rules could never match them. ALL queries now go through Sockrocket's
        # listener, whose china-split serves domestic names from
        # 223.5.5.5/119.29.29.29 itself.
        if service restart_dnsmasq >/dev/null 2>&1; then
            # Some firmware builds return from rc before dnsmasq.postconf
            # rewrites the main config; others are already done. Poll the
            # result instead of assuming either timing. A synchronous build
            # matches on the first read and does not sleep.
            if merlin_3006; then
                wait_limit="${SOCKROCKET_DNS_ISOLATION_WAIT:-15}"
                # rc often returns before dnsmasq.postconf, and a later writer
                # can put the firmware servers-file back. Apply the hook's
                # edit here as well so one missed callback does not restart
                # dnsmasq again and drop the LAN onto a poisoned resolver.
                local main_config private_file helper main_pid helper_rc saw_helper
                private_file="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
                helper="${SOCKROCKET_DNS_POSTCONF_SCRIPT:-$SOCKROCKET_DIR/scripts/dnsmasq-postconf.sh}"
                saw_helper=0
                main_config=$(main_dnsmasq_config)
                log_info "Merlin 3006 main config ${main_config}: $(main_dns_state "$main_config" "$dns_port")"
                while :; do
                    main_config=$(main_dnsmasq_config)
                    if main_dns_isolated "$main_config" "$dns_port"; then
                        isolated=1
                        if [ "$saw_helper" = 1 ]; then
                            ok "Merlin 3006 main dnsmasq isolated by control script (${main_config}: $(main_dns_state "$main_config" "$dns_port"))"
                        else
                            ok "Merlin 3006 main dnsmasq isolated after restart (${main_config}: $(main_dns_state "$main_config" "$dns_port"))"
                        fi
                        break
                    fi
                    helper_rc=0
                    if [ -x "$helper" ] && [ -s "$private_file" ]; then
                        "$helper" "$main_config" >/dev/null 2>&1 || helper_rc=$?
                        if [ "$saw_helper" = 0 ]; then
                            log_info "Merlin 3006 DNS helper exit ${helper_rc} on ${main_config}"
                            saw_helper=1
                        fi
                        if main_dns_isolated "$main_config" "$dns_port"; then
                            isolated=1
                            main_pid=$(cat "${SOCKROCKET_DNS_MAIN_PID:-/var/run/dnsmasq.pid}" 2>/dev/null)
                            case "$main_pid" in
                                ''|*[!0-9]*) ;;
                                *) kill -HUP "$main_pid" 2>/dev/null || true ;;
                            esac
                            ok "Merlin 3006 main dnsmasq isolated by control script (${main_config}: $(main_dns_state "$main_config" "$dns_port"))"
                            break
                        fi
                    elif [ "$saw_helper" = 0 ]; then
                        log_info "Merlin 3006 DNS helper not run on ${main_config}"
                        saw_helper=1
                    fi
                    [ "$waited" -ge "$wait_limit" ] && break
                    waited=$((waited + 1))
                    sleep 1
                done
                if [ "$isolated" != 1 ]; then
                    if [ ! -s "$main_config" ]; then
                        log_error "Merlin 3006 main dnsmasq config was not written (${main_config})"
                    else
                        log_error "Merlin 3006 main dnsmasq did not use isolated upstream (${main_config}: $(main_dns_state "$main_config" "$dns_port"))"
                    fi
                    update_dnsmasq stop
                    return 1
                fi
            fi
            if ! pin_dns_upstream; then
                update_dnsmasq stop
                return 1
            fi
            # Also force-capture LAN DNS that bypasses the router entirely
            # (devices hard-coded to 8.8.8.8 & friends — GFW poisons those
            # UDP answers on the wire). Redirect every LAN :53 query into
            # this dnsmasq so the china-split above applies to them too.
            if ! "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start >/dev/null 2>&1; then
                update_dnsmasq stop
                return 1
            fi
            ok "dnsmasq rules applied (DNS port: $dns_port, all queries via Sockrocket listener)"
        else
            # A malformed/unsupported snippet can prevent dnsmasq from
            # starting at all. Remove our contribution and restore direct
            # upstreams before retrying the firmware service.
            log_error "dnsmasq restart failed — rolling back Sockrocket DNS config"
            update_dnsmasq stop
            return 1
        fi
    elif [ "$action" = "stop" ]; then
        local changed=0 resolv_file
        [ -f "$SOCKROCKET_DNSMASQ" ] && changed=1
        [ -f "${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}" ] && changed=1
        resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
        grep -q '^server=127.0.0.1#' "$resolv_file" 2>/dev/null && changed=1
        rm -f "$SOCKROCKET_DNSMASQ"
        rm -f "${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
        [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
            "$SOCKROCKET_DIR/scripts/iptables.sh" dns-stop >/dev/null 2>&1 || true
        # Unpin BEFORE restarting dnsmasq, so the restarted daemon reads the
        # restored ISP upstreams instead of our now-dead listener.
        unpin_dns_upstream || return 1
        # Firmware dnsmasq drops privileges before reading its servers-file.
        # Only credentials/backups are private; the runtime upstream file must
        # remain readable by the DNS service, including legacy stop states.
        if [ -f "$resolv_file" ]; then
            chmod 644 "$resolv_file" || return 1
        fi
        # The guard may retry fail-open every few seconds. Restarting an
        # already restored dnsmasq on each retry creates a DNS outage loop.
        if [ "$changed" = "1" ]; then
            service restart_dnsmasq 2>/dev/null || {
                log_warn "dnsmasq restart failed during cleanup"
                return 1
            }
            ok "dnsmasq rules removed"
        fi
    fi
}

# Activate routing before DNS: fake-IP answers require a working data path.
activate_startup_network() {
    if [ "$want_proxy" = "true" ]; then
        if ! wait_for_tun 20 ||
            ! "$SOCKROCKET_DIR/scripts/iptables.sh" start >> "$SOCKROCKET_LOG" 2>&1; then
            enter_lan_degraded "startup TUN/routing setup failed"
            return 1
        fi
    fi
    if [ "$want_dns" = "true" ]; then
        if ! wait_for_dns 15 || ! update_dnsmasq start; then
            enter_lan_degraded "startup DNS setup failed"
            return 1
        fi
    elif [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]; then
        update_dnsmasq stop
    fi
}

# ── Start ─────────────────────────────────────────────────────────────────────
do_start() {
    ensure_nat_hook
    sync_softcenter_state
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

    # Web UI page on standard Merlin: /www may be tmpfs and wiped on reboot.
    # Always refresh from the durable copy so deploys are not left stale.
    if [ -f "$SOCKROCKET_DIR/webui/sockrocket.asp" ]; then
        if mkdir -p /www/ext/sockrocket 2>/dev/null; then
            cp "$SOCKROCKET_DIR/webui/sockrocket.asp" /www/ext/sockrocket/sockrocket.asp 2>/dev/null || true
        fi
        mkdir -p /tmp/var/wwwext/sockrocket 2>/dev/null || true
        cp "$SOCKROCKET_DIR/webui/sockrocket.asp" /tmp/var/wwwext/sockrocket/sockrocket.asp 2>/dev/null || true
    fi
    write_api_js "$(api_port)"

    # koolcenter self-heal: Softcenter menu opens Module_sockrocket.asp.
    # Always overwrite from the durable webui so UI updates are not stuck
    # on the first-install copy (deploy used to miss this path).
    if [ -d /koolshare/scripts ] && [ -f /koolshare/scripts/base.sh ]; then
        local KS_WEBS="/koolshare/webs"
        if [ -f "$SOCKROCKET_DIR/webui/sockrocket.asp" ]; then
            mkdir -p "$KS_WEBS" /jffs/.koolshare/webs 2>/dev/null || true
            cp "$SOCKROCKET_DIR/webui/sockrocket.asp" "$KS_WEBS/Module_sockrocket.asp" 2>/dev/null || true
            cp "$SOCKROCKET_DIR/webui/sockrocket.asp" /jffs/.koolshare/webs/Module_sockrocket.asp 2>/dev/null || true
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
    # freezes Softcenter ("loading data, retry later") and can blackhole SSH.
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
    # Default OFF: match config.yaml.template and Softcenter-safe install.
    want_dns=$(conf_bool dns_hijack false)

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

    # DNS hijack WITHOUT TUN is safe: Fake-IP is only enabled when the
    # daemon is started with --tun. SOCKS-mode hijack serves short-TTL real
    # IPs (china-split) so closing TUN does not push clients onto ISP DNS
    # (long TTLs → ~1min bare-IP blackhole after re-enable).

    local launch_tun="$want_proxy"
    # Fail-open marker: keep the daemon up for recovery probes,
    # but do not re-hijack LAN until try_recover_lan_proxy clears the marker.
    if [ -f "${SOCKROCKET_DIR}/.lan_degraded" ]; then
        if [ "$want_proxy" = "true" ] || [ "$want_dns" = "true" ]; then
            warn "LAN degraded ($(cat "${SOCKROCKET_DIR}/.lan_degraded" 2>/dev/null)) — starting without transparent proxy/DNS hijack"
        fi
        want_proxy=false
        want_dns=false
    fi

    # TUN must be up BEFORE handing --tun to sockrocket-cli.
    if [ "$launch_tun" = "true" ] && ! ensure_tun; then
        log_error "Failed to enable transparent proxy: TUN device unavailable (tried modprobe/insmod/mknod)"
        want_proxy=false
        launch_tun=false
        # Keep DNS hijack if wanted — SOCKS mode has no Fake-IP.
    fi

    # Core dumps must be enabled in THIS shell: the daemon inherits ulimit
    # from its parent. core_pattern is system-wide and idempotent.
    enable_core_dumps

    # Even during fail-open, create the requested TUN device without
    # installing LAN routes. Otherwise recovery cannot restore transparent
    # proxying: a SOCKS-only process never creates the missing interface.
    if [ "$launch_tun" = "true" ]; then
        start_daemon_supervised 1
        [ "$(get_config_value mode)" != "tun" ] && sed -i 's/^mode:.*/mode: "tun"/' "$SOCKROCKET_CONF" 2>/dev/null
    else
        start_daemon_supervised 0
        [ "$(get_config_value mode)" != "socks" ] && sed -i 's/^mode:.*/mode: "socks"/' "$SOCKROCKET_CONF" 2>/dev/null
    fi
    # Supervisor writes SOCKROCKET_PID; poll briefly instead of a fixed 1s sleep.
    local ready=0
    while [ "$ready" -lt 3 ]; do
        is_running && break
        sleep 1
        ready=$((ready + 1))
    done
    if is_running; then
        ok "Sockrocket started (PID $(cat "$SOCKROCKET_PID"))"
        sync_softcenter_state
        # Keep recovery active even when synchronous activation fails below.
        start_guard
        # Softcenter offline install must not block 20–35s on TUN/DNS waits —
        # that freezes the software-center UI ("loading data"). FAST_START
        # applies rules in the background; normal starts wait as before.
        if [ "${SOCKROCKET_FAST_START:-0}" = "1" ]; then
            (
                trap '' HUP INT TERM
                trap 'release_lock' EXIT
                # This child must not inherit ownership of the parent's
                # lock, or race a later stop while waiting for DNS/TUN.
                LOCK_HELD=0
                acquire_lock 40 || exit 1
                [ -f "$SOCKROCKET_DIR/stopped" ] && exit 0
                [ -f "$DEGRADED_FILE" ] && exit 0
                is_running || exit 0
                [ "$(count_section nodes)" != "0" ] || exit 0
                want_proxy=$(conf_bool transparent_proxy true)
                want_dns=$(conf_bool dns_hijack false)
                activate_startup_network || exit 1
                if [ "$want_proxy" != "true" ] && \
                    iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                        "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
                fi
            ) >> "$SOCKROCKET_LOG" 2>&1 &
        else
            activate_startup_network || return 1
            if [ "$want_proxy" != "true" ] && \
                iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
            fi
        fi
        setup_cron
        try_recover_lan_proxy
    else
        log_error "Sockrocket failed to start. Last log lines:"
        tail -20 "$SOCKROCKET_LOG" 2>/dev/null | while IFS= read -r line; do
            log_error "  $line"
        done
        rm -f "$SOCKROCKET_PID"
        # Daemon failed with possible leftover intent — don't leave LAN hijacked.
        enter_lan_degraded "start failed"
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
    local restore_rc=0
    # Transparent proxy + DNS NAT (script may already be gone on uninstall)
    if [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ]; then
        "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1 || restore_rc=1
        "$SOCKROCKET_DIR/scripts/iptables.sh" dns-stop >> "$SOCKROCKET_LOG" 2>&1 || true
    else
        iptables -t mangle -D PREROUTING -j SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -t mangle -F SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -t mangle -X SOCKROCKET_MANGLE 2>/dev/null || true
        iptables -D FORWARD -j SOCKROCKET_UDP_GUARD 2>/dev/null || true
        iptables -F SOCKROCKET_UDP_GUARD 2>/dev/null || true
        iptables -X SOCKROCKET_UDP_GUARD 2>/dev/null || true
        iptables -t mangle -F SOCKROCKET_DEVICES 2>/dev/null || true
        iptables -t mangle -X SOCKROCKET_DEVICES 2>/dev/null || true
        iptables -t nat -D PREROUTING -j SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -F SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -X SOCKROCKET_DNS 2>/dev/null || true
        iptables -t nat -D PREROUTING -j SOCKROCKET_PREROUTING 2>/dev/null || true
        iptables -t nat -F SOCKROCKET_PREROUTING 2>/dev/null || true
        iptables -t nat -X SOCKROCKET_PREROUTING 2>/dev/null || true
        local v6_attempt=0
        while [ "$v6_attempt" -lt 32 ] && ip6tables -D FORWARD -j SOCKROCKET_IPV6_GUARD 2>/dev/null; do
            v6_attempt=$((v6_attempt + 1))
        done
        if ip6tables -C FORWARD -j SOCKROCKET_IPV6_GUARD 2>/dev/null; then restore_rc=1; fi
        ip6tables -F SOCKROCKET_IPV6_GUARD 2>/dev/null || true
        ip6tables -X SOCKROCKET_IPV6_GUARD 2>/dev/null || true
        local v6_table v6_chain
        for v6_table in mangle nat; do
            if [ "$v6_table" = mangle ]; then v6_chain=SOCKROCKET_IPV6_MANGLE; else v6_chain=SOCKROCKET_IPV6_DNS; fi
            v6_attempt=0
            while [ "$v6_attempt" -lt 32 ] && ip6tables -t "$v6_table" -D PREROUTING -j "$v6_chain" 2>/dev/null; do v6_attempt=$((v6_attempt+1)); done
            if ip6tables -t "$v6_table" -C PREROUTING -j "$v6_chain" 2>/dev/null; then restore_rc=1; fi
            ip6tables -t "$v6_table" -F "$v6_chain" 2>/dev/null || true
            ip6tables -t "$v6_table" -X "$v6_chain" 2>/dev/null || true
        done
        v6_attempt=0
        while [ "$v6_attempt" -lt 32 ] && ip6tables -D FORWARD -j SOCKROCKET_IPV6_FORWARD 2>/dev/null; do v6_attempt=$((v6_attempt+1)); done
        if ip6tables -C FORWARD -j SOCKROCKET_IPV6_FORWARD 2>/dev/null; then restore_rc=1; fi
        ip6tables -F SOCKROCKET_IPV6_FORWARD 2>/dev/null || true
        ip6tables -X SOCKROCKET_IPV6_FORWARD 2>/dev/null || true
        v6_attempt=0
        while [ "$v6_attempt" -lt 32 ] && ip -6 rule del fwmark 0x4765 table 5370 2>/dev/null; do v6_attempt=$((v6_attempt+1)); done
        if ip -6 rule show 2>/dev/null | grep -Eq 'fwmark 0x4765(/0xffffffff)? .*lookup 5370([[:space:]]|$)'; then restore_rc=1; fi
        ip -6 route del default dev sockrocket-tun table 5370 2>/dev/null || true
        iptables -D FORWARD -i sockrocket-tun -j ACCEPT 2>/dev/null || true
        ip rule  del fwmark 0x4765 table 5370 2>/dev/null || true
        ip route flush table 5370 2>/dev/null || true
        ip rule  del fwmark 0x4765 table 100 2>/dev/null || true
    fi
    # Full restore: remove only our Fake-IP routes.
    ip route del 198.18.0.0/16 dev sockrocket-tun 2>/dev/null || true
    ip route del blackhole 198.18.0.0/16 2>/dev/null || true
    ip route del unreachable 198.18.0.0/16 2>/dev/null || true
    ip -6 route del 2001:2:0:4765::/96 dev sockrocket-tun 2>/dev/null || true

    # dnsmasq hijack file + upstream pin
    update_dnsmasq stop || restore_rc=1

    # Belt: if pin survived, rebuild ISP upstreams from backup or /etc/resolv.conf
    local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
    if grep -q "^server=127.0.0.1#" "$resolv_file" 2>/dev/null; then
        log_warn "DNS pin still present after cleanup — force-restoring ISP upstreams"
        unpin_dns_upstream || restore_rc=1
        kill -HUP "$(pidof dnsmasq)" 2>/dev/null || true
    fi

    # Drop TUN device left by --tun so no stale routes linger
    ip link del sockrocket-tun 2>/dev/null || true

    # Restore DNS/routing on every stop, but retain the private Fake-IP map.
    # LAN clients can keep DNS answers across stop/start, upgrade or reboot;
    # deleting the map would invalidate or reassign their cached addresses.
    # This cache does not install any proxy setting and is removed on uninstall.
    if [ -f "$SOCKROCKET_DIR/stopped" ]; then
        rm -f "$SOCKROCKET_DIR/tun-routes.yaml" 2>/dev/null || true
    fi
    # SIGHUP clears dnsmasq's cache. Restart above already does that when our
    # snippet was installed; this covers answers left after a crash.
    if pidof dnsmasq >/dev/null 2>&1; then
        kill -HUP $(pidof dnsmasq) 2>/dev/null || true
    fi

    # OOM under TUN/ipstack has been observed to kill dropbear and leave the
    # router unreachable over SSH while HTTP still answers. Best-effort
    # revive management access whenever we restore the direct path.
    ensure_sshd
    if grep -q '^server=127.0.0.1#' "$resolv_file" 2>/dev/null; then
        restore_rc=1
    fi
    return "$restore_rc"
}

# Restart dropbear/sshd when the configured SSH port is not listening.
# Safe no-op when SSH is intentionally disabled (nvram sshd_enable=0).
ensure_sshd() {
    local en port
    en=$(nvram get sshd_enable 2>/dev/null || echo "")
    [ "$en" = "0" ] && return 0
    port=$(nvram get sshd_port 2>/dev/null || echo "")
    port=${port:-22}
    # Already listening?
    if netstat -ln 2>/dev/null | grep -qE ":${port}([[:space:]]|$)" \
        || netstat -ln 2>/dev/null | grep -qE "[:.]${port}[[:space:]]"; then
        return 0
    fi
    log_warn "SSH port ${port} not listening — restarting dropbear/sshd"
    if service restart_sshd >/dev/null 2>&1; then
        sleep 1
    elif which dropbear >/dev/null 2>&1; then
        killall dropbear 2>/dev/null || true
        dropbear -p "$port" >/dev/null 2>&1 || dropbear >/dev/null 2>&1 || true
        sleep 1
    fi
    if netstat -ln 2>/dev/null | grep -qE ":${port}([[:space:]]|$)"; then
        ok "SSH restored on port ${port}"
    else
        log_warn "SSH still down on port ${port} — reboot the router if management stays unreachable"
    fi
}

do_stop() {
    # Tear down BEFORE the early return: a crashed daemon (dead PID in
    # sockrocket.pid) used to skip all cleanup, leaving the iptables MARK rules and
    # the dnsmasq hijack (catch-all → 127.0.0.1:5300) live with nothing
    # listening — a LAN-wide DNS blackhole the Web UI could not clear.
    #
    # Stamp stopped FIRST so the supervisor does not treat our kill as a crash
    # and immediately respawn / fail-open race.
    : > "$SOCKROCKET_DIR/stopped"
    sync_softcenter_state
    stop_guard
    stop_supervisor

    if is_running; then
        info "Stopping Sockrocket (API bridge keeps running so the Web UI can start it again)..."
        local pid
        pid=$(cat "$SOCKROCKET_PID")
        kill "$pid" 2>/dev/null
        # Daemon handles SIGTERM (and used to ignore it → full 8s wait every
        # TUN toggle). Keep a short grace window then SIGKILL.
        local wait=0
        while kill -0 "$pid" 2>/dev/null && [ "$wait" -lt 3 ]; do
            sleep 1; wait=$((wait + 1))
        done
        kill -0 "$pid" 2>/dev/null && kill -9 "$pid" 2>/dev/null || true
    else
        info "Sockrocket process not running — cleaning up leftover rules anyway"
    fi
    rm -f "$SOCKROCKET_PID"

    kill_stray_daemons

    local cleanup_rc=0
    restore_direct_network || cleanup_rc=1
    remove_cron
    restore_core_pattern
    prune_core_dumps
    rm -f "$DEGRADED_FILE" "$WOUT_FAILS_FILE"

    if [ "$cleanup_rc" != 0 ]; then
        log_error "Sockrocket stopped, but network cleanup failed; recovery state retained"
        return 1
    fi
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

# proxy-off: tear down transparent-proxy (iptables/TUN) but KEEP DNS hijack
# when dns_hijack=true.
#
# Fake-IP only exists with --tun. Restarting without --tun makes the DNS
# listener hand out short-TTL *real* IPs — LAN keeps working, and re-enabling
# TUN recovers in seconds (no ISP long-TTL hang). Callers write
# transparent_proxy=false FIRST; leave dns_hijack as-is unless the user
# explicitly turned DNS off.
do_proxy_off() {
    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
    local rc=0
    iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK" && rc=1
    # A daemon started with --tun keeps its TUN stack (and Fake-IP pool) up
    # even after the steering rules are gone; restart in plain mode so Fake-IP
    # stops and short-TTL real answers take over.
    local pid
    pid=$(cat "$SOCKROCKET_PID" 2>/dev/null)
    if [ -n "$pid" ] \
        && tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null | grep -q -- "--tun"; then
        do_stop
        do_start
    fi
    # DNS hijack is independent of TUN. do_stop tears the hijack down; put it
    # back whenever the key is still on so LAN never falls through to ISP DNS.
    if [ "$(conf_bool dns_hijack false)" = "true" ]; then
        if wait_for_dns 10; then
            update_dnsmasq start || rc=1
        else
            log_warn "proxy-off: DNS port not listening — hijack left off"
            rc=1
        fi
    fi
    ensure_sshd
    return $rc
}

# ── Status ────────────────────────────────────────────────────────────────────
do_status() {
    sync_softcenter_state
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
    # Cron and WebUI use the same configuration transaction and fetch logic.
    # Do not stop LAN forwarding while downloading; the CGI preserves manual
    # stop and acquires the lifecycle lock only if applying a running update.
    local result="$SOCKROCKET_DIR/subscription-update.$$.tmp" status=0
    umask 077
    SOCKROCKET_LOCAL_RPC=1 REQUEST_METHOD=POST QUERY_STRING=action=update_subs \
        "$SOCKROCKET_BIN" cgi </dev/null > "$result" 2>> "$SOCKROCKET_LOG" || status=$?
    if [ "$status" -eq 0 ] && grep -q '"ok":true' "$result"; then
        rm -f "$result"
        ok "Subscriptions updated; service state preserved"
        return 0
    fi
    rm -f "$result"
    log_error "Subscription update failed; existing service state retained (see WebUI for details)"
    return 1
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
    pidof dnsmasq >/dev/null 2>&1 && dns_running="YES"
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
    # Repair existing artifacts before appending private diagnostic content.
    if [ -f "$SOCKROCKET_CRASH_LOG" ]; then
        chmod 600 "$SOCKROCKET_CRASH_LOG" || return 1
    fi
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

# ── Second-scale supervisor / guard ───────────────────────────────────────────
# Cron (1 min) is only a safety net. Real recovery must be seconds:
#   - supervisor waits on the daemon; on exit → immediate LAN fail-open + restart
#   - guard polls every 5s for dead DNS port / recover-from-degraded
#   - half-dead (pid alive, DNS gone) → kill daemon so supervisor respawns
#     (fail-open alone left LAN on direct path while a zombie process sat idle)

# Kill a still-alive daemon without writing the manual-stop marker, so the
# supervised waiter respawns a fresh process. Used when DNS/:5300 (or TUN
# via the CLI's own fatal exit) dies while the PID file still looks healthy.
kill_half_dead_daemon() {
    local reason="$1" pid
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    pid=$(cat "$SOCKROCKET_PID" 2>/dev/null)
    if [ -z "$pid" ] || ! kill -0 "$pid" 2>/dev/null; then
        return 0
    fi
    log_error "Half-dead daemon (pid $pid): ${reason} — killing for supervised restart"
    kill "$pid" 2>/dev/null || true
}

stop_supervisor() {
    local pid
    pid=$(cat "$SUPERVISOR_PID" 2>/dev/null)
    # The waiter masks TERM (`trap '' HUP INT TERM`); SIGKILL immediately.
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    rm -f "$SUPERVISOR_PID"
}

stop_guard() {
    local pid
    pid=$(cat "$GUARD_PID_FILE" 2>/dev/null)
    # Same as supervisor: the guard loop ignores TERM.
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    rm -f "$GUARD_PID_FILE"
}

# Launch sockrocket-cli under a waiter that fail-opens the LAN the instant the
# process exits (no waiting for cron), then respawns in a loop until stopped.
start_daemon_supervised() {
    local use_tun="$1"
    stop_supervisor
    (
        LOCK_HELD=0
        trap - EXIT
        trap '' HUP INT TERM
        local fast_fails=0
        while [ ! -f "$SOCKROCKET_DIR/stopped" ]; do
            if [ "$use_tun" = "1" ]; then
                nice -n 3 "$SOCKROCKET_BIN" "$SOCKROCKET_CONF" --tun >> "$SOCKROCKET_LOG" 2>&1 &
            else
                nice -n 3 "$SOCKROCKET_BIN" "$SOCKROCKET_CONF" >> "$SOCKROCKET_LOG" 2>&1 &
            fi
            local dpid=$!
            echo "$dpid" > "$SOCKROCKET_PID"
            local started
            started=$(date +%s 2>/dev/null || echo 0)
            # Poll death every 1s. Only fail-open on "DNS port lost" after we
            # have observed the port UP at least once — otherwise startup races
            # (TUN/DNS bind takes a few seconds) false-trigger degrade.
            local dns_seen_up=0 dns_missing_checks=0 state_published=0
            while kill -0 "$dpid" 2>/dev/null; do
                if [ "$state_published" = 0 ] && is_running; then
                    sync_softcenter_state
                    state_published=1
                fi
                if [ -f "$SOCKROCKET_DIR/stopped" ]; then
                    wait "$dpid" 2>/dev/null
                    exit 0
                fi
                if [ "$(conf_bool dns_hijack false)" = "true" ]; then
                    local dns_port
                    dns_port=$(get_config_value dns_port)
                    dns_port=${dns_port:-5300}
                    if netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)"; then
                        dns_seen_up=1
                        dns_missing_checks=0
                    elif [ "$dns_seen_up" = "1" ] && [ ! -f "$DEGRADED_FILE" ]; then
                        if dns_listener_healthy; then
                            dns_missing_checks=0
                        else
                            dns_missing_checks=$((dns_missing_checks + 1))
                        fi
                        # Confirm repeated absence AND failed protocol probes.
                        # A single netstat miss must not interrupt LAN traffic.
                        if [ "$dns_missing_checks" -lt 3 ]; then
                            sleep 1
                            continue
                        fi
                        echo "daemon DNS port lost while pid $dpid still alive" > "$DEGRADED_FILE"
                        sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" network-restore >> "$SOCKROCKET_LOG" 2>&1 || true
                        log_error "LAN fail-open: DNS port lost while pid $dpid still alive"
                        # Fail-open alone is not enough: the process can keep
                        # SOCKS up with a dead DNS/TUN stack. Kill so this
                        # wait loop ends and the outer supervisor respawns.
                        log_error "Half-dead daemon (pid $dpid): DNS port lost — killing for supervised restart"
                        kill "$dpid" 2>/dev/null || true
                    fi
                fi
                sleep 1
            done
            wait "$dpid" 2>/dev/null
            local rc=$?
            if [ "$(cat "$SOCKROCKET_PID" 2>/dev/null)" = "$dpid" ]; then
                rm -f "$SOCKROCKET_PID"
            fi
            sync_softcenter_state
            if [ -f "$SOCKROCKET_DIR/stopped" ]; then
                exit 0
            fi
            local ended lived
            ended=$(date +%s 2>/dev/null || echo 0)
            lived=$((ended - started))
            if [ "$lived" -lt 5 ]; then
                fast_fails=$((fast_fails + 1))
            else
                fast_fails=0
            fi
            echo "daemon exited rc=$rc" > "$DEGRADED_FILE"
            sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" network-restore >> "$SOCKROCKET_LOG" 2>&1 || true
            log_error "Daemon exited (pid $dpid rc=$rc) — fail-open LAN immediately"
            # A config the process rejects at startup must not spin forever.
            # The first two exits still retry; after that the LAN stays direct
            # until the config file changes or the operator stops the service.
            if [ "$fast_fails" -ge 3 ]; then
                log_error "Daemon failed at startup $fast_fails times — LAN stays direct until the config changes"
                local stamp now
                stamp=$(cksum "$SOCKROCKET_CONF" 2>/dev/null || wc -c < "$SOCKROCKET_CONF")
                while [ ! -f "$SOCKROCKET_DIR/stopped" ]; do
                    sleep 5
                    now=$(cksum "$SOCKROCKET_CONF" 2>/dev/null || wc -c < "$SOCKROCKET_CONF")
                    if [ "$now" != "$stamp" ]; then
                        fast_fails=0
                        break
                    fi
                done
                continue
            fi
            sleep 2
        done
    ) &
    echo $! > "$SUPERVISOR_PID"
}

# 5-second poller: dead DNS listener while hijack is intended → fail-open;
# degraded + healthy probe → re-arm proxy path.
start_guard() {
    stop_guard
    (
        LOCK_HELD=0
        trap - EXIT
        trap '' HUP INT TERM
        local dns_miss=0 dns_tick=0
        while true; do
            sleep 3
            dns_tick=$((dns_tick + 1))
            [ -f "$SOCKROCKET_DIR/stopped" ] && exit 0

            if [ -f "$DEGRADED_FILE" ]; then
                if is_running; then
                    # Prefer script entry so recover works even if functions
                    # were not inherited by this subshell.
                    sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" recover >> "$SOCKROCKET_LOG" 2>&1 || true
                fi
                continue
            fi

            if ! is_running; then
                echo "daemon not running (guard)" > "$DEGRADED_FILE"
                sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" network-restore >> "$SOCKROCKET_LOG" 2>&1 || true
                continue
            fi

            if [ "$(conf_bool dns_hijack false)" = "true" ]; then
                local dns_port
                dns_port=$(get_config_value dns_port)
                dns_port=${dns_port:-5300}
                # Probe answers every 15s; still notice a lost port each tick.
                if netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)" &&
                    [ "$((dns_tick % 5))" -ne 0 ]; then
                    continue
                fi
                if dns_listener_healthy; then
                    dns_miss=0
                else
                    dns_miss=$((dns_miss + 1))
                    if [ "$dns_miss" -ge 2 ]; then
                        echo "DNS port ${dns_port} dead (guard)" > "$DEGRADED_FILE"
                        sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" network-restore >> "$SOCKROCKET_LOG" 2>&1 || true
                        sh "$SOCKROCKET_DIR/scripts/sockrocket.sh" kill-half-dead "DNS port ${dns_port} dead (guard)" >> "$SOCKROCKET_LOG" 2>&1 || true
                        dns_miss=0
                    fi
                fi
            else
                dns_miss=0
            fi
        done
    ) &
    echo $! > "$GUARD_PID_FILE"
}

# ── LAN fail-open / recover ───────────────────────────────────────────────────
# User toggles in config.yaml express INTENT and must survive failures.
# Runtime degradation tears down DNS hijack + transparent rules so the LAN
# returns to direct connectivity, then the watchdog/guard re-applies them once
# the daemon and outbound probe are healthy again.

enter_lan_degraded() {
    local reason="$1"
    # Idempotent stamp; always tear down so a second call still clears rules.
    echo "$reason" > "$DEGRADED_FILE" 2>/dev/null
    log_error "LAN fail-open: ${reason} — restoring direct DNS/routing (config intent kept)"
    update_dnsmasq stop
    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
        "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
    ensure_sshd
}

# True when SOCKS/HTTP can complete a basic outbound HTTP probe (node path alive).
outbound_probe_ok() {
    local http_port
    http_port=$(get_config_value http_port)
    http_port=${http_port:-1087}
    if which curl >/dev/null 2>&1; then
        curl -x "http://127.0.0.1:${http_port}" -fsS -m 5 -o /dev/null \
            "http://www.gstatic.com/generate_204" 2>/dev/null && return 0
        return 1
    fi
    if which timeout >/dev/null 2>&1; then
        timeout 2 sh -c "echo | nc -w1 127.0.0.1 $http_port >/dev/null 2>&1"
    else
        nc -w1 127.0.0.1 "$http_port" </dev/null >/dev/null 2>&1
    fi
}

# Busybox shells on 32-bit routers must not multiply MemTotal by 100 in
# shell arithmetic. awk also supports older kernels without MemAvailable.
memory_used_pct() {
    awk '
        /^MemTotal:/ { total=$2 }
        /^MemAvailable:/ { available=$2; have_available=1 }
        /^MemFree:/ { free_mem=$2 }
        /^Buffers:/ { buffers=$2 }
        /^Cached:/ { cached=$2 }
        END {
            if (total > 0) {
                if (!have_available) available=free_mem+buffers+cached
                printf "%.0f\n", (total-available)*100/total
            }
        }
    ' "${SOCKROCKET_MEMINFO_FILE:-/proc/meminfo}" 2>/dev/null
}

recover_waiting() {
    local now until
    [ -f "$RECOVER_AFTER_FILE" ] || return 1
    now=$(date +%s 2>/dev/null || echo 0)
    until=$(cat "$RECOVER_AFTER_FILE" 2>/dev/null)
    case "$until" in ''|*[!0-9]*) return 1 ;; esac
    [ "$now" -lt "$until" ]
}

hold_recover() {
    local now
    now=$(date +%s 2>/dev/null || echo 0)
    echo $((now + ${1:-60})) > "$RECOVER_AFTER_FILE" 2>/dev/null || true
}

# Re-apply transparent proxy + DNS hijack from config after a fail-open, once
# the daemon and outbound path look healthy again.
try_recover_lan_proxy() {
    [ -f "$DEGRADED_FILE" ] || return 0
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    if recover_waiting; then
        return 0
    fi
    is_running || return 0
    [ "$(count_section nodes)" != "0" ] || return 1
    # Hysteresis: watchdog degrades at 85%; only re-arm below 80%.
    # Both cron and the fast guard call this function.
    local mem_pct
    mem_pct=$(memory_used_pct)
    [ -z "$mem_pct" ] || [ "$mem_pct" -lt 80 ] || return 1

    local dns_port
    dns_port=$(get_config_value dns_port)
    dns_port=${dns_port:-5300}
    # Only require DNS port when hijack is intended.
    if [ "$(conf_bool dns_hijack false)" = "true" ]; then
        if ! netstat -lun 2>/dev/null | grep -qE ":${dns_port}([[:space:]]|$)"; then
            return 0
        fi
    fi
    outbound_probe_ok || return 0

    local want_proxy want_dns
    want_proxy=$(conf_bool transparent_proxy true)
    want_dns=$(conf_bool dns_hijack false)

    log_warn "LAN recovering — re-applying proxy path (was: $(cat "$DEGRADED_FILE" 2>/dev/null))"
    if [ "$want_proxy" = "true" ]; then
        # Never enable fake-IP DNS or clear the marker without a working
        # transparent path. A running SOCKS listener alone is insufficient.
        wait_for_tun 1 || return 1
        [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] || return 1
        if ! "$SOCKROCKET_DIR/scripts/iptables.sh" start >> "$SOCKROCKET_LOG" 2>&1; then
            enter_lan_degraded "recovery firewall setup failed"
            return 1
        fi
    fi
    if [ "$want_dns" = "true" ]; then
        if ! update_dnsmasq start; then
            hold_recover 60
            enter_lan_degraded "recovery DNS setup failed"
            return 1
        fi
    fi
    rm -f "$DEGRADED_FILE" "$WOUT_FAILS_FILE" "$RECOVER_AFTER_FILE"
    ok "LAN proxy path restored after fail-open"
}

# ── Watchdog ──────────────────────────────────────────────────────────────────
# nat-start runs when the firmware rebuilds NAT without a full firewall pass.
# DNS redirects live in the nat table, so restore them immediately.
do_nat_restore() {
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    [ -f "$DEGRADED_FILE" ] && return 0
    is_running || return 0
    [ "$(count_section nodes)" != "0" ] || return 0
    if [ "$(conf_bool dns_hijack false)" = "true" ] && \
        [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]; then
        wait_for_dns 1 || return 1
        "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start || return 1
    fi
    if [ "$(get_config_value ipv6_mode)" = "dual_stack" ] \
        && [ "$(conf_bool transparent_proxy true)" = "true" ] \
        && [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ]; then
        "$SOCKROCKET_DIR/scripts/iptables.sh" ipv6-apply || return 1
    fi
    return 0
}

do_firewall_restore() {
    # Firmware firewall rebuilds must respect the same fail-open and stop
    # intent as the watchdog. Do not re-arm merely because a TUN exists.
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    [ -f "$DEGRADED_FILE" ] && return 0
    is_running || return 0
    [ "$(count_section nodes)" != "0" ] || return 0
    if [ "$(conf_bool transparent_proxy true)" = "true" ]; then
        wait_for_tun 1 || return 1
        "$SOCKROCKET_DIR/scripts/iptables.sh" start || {
            enter_lan_degraded "firewall rebuild failed"
            return 1
        }
    fi
    if [ "$(conf_bool dns_hijack false)" = "true" ] && \
        [ -f "${SOCKROCKET_DNSMASQ_DIR:-/jffs/configs/dnsmasq.d}/sockrocket.conf" ]; then
        wait_for_dns 1 || return 1
        "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start || return 1
    fi
    return 0
}

# PPPoE reconnects and DHCP/static WAN transitions can change the firmware's
# routing policy independently of firewall-start. Reapply our private TUN
# policy after the WAN is connected; this neither binds to ppp0/eth* nor
# restarts the daemon, so the same path works for every dial-up mode.
do_wan_restore() {
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    [ -f "$DEGRADED_FILE" ] && return 0
    is_running || return 0
    [ "$(count_section nodes)" != "0" ] || return 0
    do_firewall_restore || return 1
    pin_dns_upstream
}

do_pin_dns_event() {
    [ -f "$DEGRADED_FILE" ] && return 0
    pin_dns_upstream
}

# DHCPv6 and router-advertisement changes can replace LAN prefixes without a
# firewall-start event. Reconcile IPv6 DNS/policy immediately; the periodic
# watchdog remains a retry path if the firmware has not finished applying it.
do_dhcpv6_restore() {
    local failed=0
    [ -f "$SOCKROCKET_DIR/stopped" ] && return 0
    [ -f "$DEGRADED_FILE" ] && return 0
    is_running || return 0
    [ "$(count_section nodes)" != "0" ] || return 0
    pin_dns_upstream || failed=1
    if [ "$(conf_bool transparent_proxy true)" = "true" ] && \
        [ "$(get_config_value ipv6_mode)" = "dual_stack" ]; then
        if [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ]; then
            "$SOCKROCKET_DIR/scripts/iptables.sh" ipv6-apply || failed=1
        else
            failed=1
        fi
    fi
    return "$failed"
}

do_watchdog() {
    local issues=0
    sync_softcenter_state

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
        # Even while stopped, revive SSH if OOM killed dropbear earlier.
        ensure_sshd
        # Manual stop already tore rules down; clear any stale degrade marker.
        rm -f "$DEGRADED_FILE" "$WOUT_FAILS_FILE"
        return
    fi

    # Memory pressure: fail-open WITHOUT rewriting user toggles so we can
    # recover automatically once memory recovers.
    local mem_pct
    mem_pct=$(memory_used_pct)
    mem_pct=${mem_pct:-0}
    if [ "$mem_pct" -ge 85 ]; then
        enter_lan_degraded "memory pressure ${mem_pct}%"
        issues=$((issues + 1))
    fi

    local healthy=1
    if ! is_running; then
        log_error "Watchdog: Sockrocket not running — restarting..."
        collect_crash_dump
        # Fail-open for the restart window so LAN is not blackholed if start fails.
        enter_lan_degraded "daemon not running"
        do_start >> "$SOCKROCKET_LOG" 2>&1
        issues=$((issues + 1))
        if is_running; then
            healthy=1
            # do_start respects .lan_degraded (SOCKS only). Recovery below
            # re-applies hijack/TP once the outbound probe passes.
        else
            healthy=0
        fi
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

        # Outbound liveness while hijack/TP are supposed to be on: consecutive
        # probe failures → fail-open; success clears the counter and may recover.
        if [ ! -f "$DEGRADED_FILE" ] \
            && { [ "$(conf_bool transparent_proxy true)" = "true" ] \
              || [ "$(conf_bool dns_hijack false)" = "true" ]; }; then
            if outbound_probe_ok; then
                rm -f "$WOUT_FAILS_FILE"
            else
                local ofails=0
                [ -f "$WOUT_FAILS_FILE" ] && ofails=$(cat "$WOUT_FAILS_FILE" 2>/dev/null)
                ofails=$((ofails + 1))
                echo "$ofails" > "$WOUT_FAILS_FILE"
                if [ "$ofails" -ge 3 ]; then
                    enter_lan_degraded "outbound probe failed ${ofails}x"
                    issues=$((issues + 1))
                else
                    log_warn "Watchdog: outbound probe failed (${ofails}/3)"
                fi
            fi
        fi

        # Self-heal a rebooted router: tun.ko never auto-loads, so if the
        # user wants transparent proxying and the device is gone, reload it
        # before reapplying the rules — but NOT while degraded (LAN must stay
        # on the direct path until probe recovers).
        if [ ! -f "$DEGRADED_FILE" ] && [ "$(conf_bool transparent_proxy true)" = "true" ]; then
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
            if [ "$(conf_bool transparent_proxy true)" != "true" ] || [ -f "$DEGRADED_FILE" ]; then
                if iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
                    log_warn "Watchdog: transparent proxy off/degraded but stale rules exist — removing"
                    [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop >> "$SOCKROCKET_LOG" 2>&1
                    issues=$((issues + 1))
                fi
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
    want_dns=$(conf_bool dns_hijack false)
    local fail_file="$SOCKROCKET_DIR/.wdns_fails"

    if [ "$(count_section nodes)" = "0" ]; then
        if [ -f "$dns_conf" ] || iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK"; then
            enter_lan_degraded "no nodes in config"
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
    elif [ -f "$DEGRADED_FILE" ]; then
        # Stay fail-open while degraded; recovery happens below.
        if [ -f "$dns_conf" ]; then
            update_dnsmasq stop
        fi
        rm -f "$fail_file"
    elif ! is_running; then
        # The restart attempt above already ran; still-dead means fail open.
        enter_lan_degraded "daemon still dead after restart"
        issues=$((issues + 1))
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
                # IPv6 DNS is retried by ipv6-apply below: rebuilding the IPv4
                # chain for it would open a bypass window on every pass.
                log_warn "Watchdog: DNS NAT chain missing — reapplying dns-start"
                [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
                    "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start >> "$SOCKROCKET_LOG" 2>&1
                issues=$((issues + 1))
            fi
            # Reconcile the main LAN upstream; 3006 must leave the shared
            # resolv.dnsmasq file for SDNs on their firmware DNS path.
            local resolv_file="${SOCKROCKET_RESOLV_FILE:-/tmp/resolv.dnsmasq}"
            merlin_3006 && resolv_file="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
            local before
            before=$(cat "$resolv_file" 2>/dev/null)
            pin_dns_upstream
            [ "$(cat "$resolv_file" 2>/dev/null)" != "$before" ] && \
                log_warn "Watchdog: main dnsmasq upstream restored to Sockrocket"
            if merlin_3006 && ! main_dns_isolated "$(main_dnsmasq_config)" "$dns_port"; then
                log_warn "Watchdog: Merlin 3006 main dnsmasq lost its isolated upstream ($(main_dnsmasq_config): $(main_dns_state "$(main_dnsmasq_config)" "$dns_port")) — restarting DNS"
                update_dnsmasq start || enter_lan_degraded "3006 DNS upstream isolation unavailable"
                issues=$((issues + 1))
            fi
        else
            local fails=0
            [ -f "$fail_file" ] && fails=$(cat "$fail_file" 2>/dev/null)
            fails=$((fails + 1))
            echo "$fails" > "$fail_file"
            if [ "$fails" -ge 3 ]; then
                enter_lan_degraded "DNS port $dns_port dead for $fails consecutive checks"
                kill_half_dead_daemon "DNS port $dns_port dead for $fails consecutive checks"
                rm -f "$fail_file"
                issues=$((issues + 1))
            else
                log_warn "Watchdog: DNS port $dns_port not listening (consecutive failure $fails/3) — keeping current DNS state"
            fi
        fi
    fi

    # Dual-stack retries in place and leaves the IPv4 proxy up if it cannot
    # finish. Compatibility only installs the reject guard once firmware IPv6
    # appears, so enabling Native does not wait for the next firewall rebuild.
    if [ ! -f "$DEGRADED_FILE" ] && [ ! -f "$SOCKROCKET_DIR/stopped" ] && is_running \
        && [ "$(conf_bool transparent_proxy true)" = "true" ] \
        && [ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ]; then
        if [ "$(get_config_value ipv6_mode)" = "dual_stack" ]; then
            "$SOCKROCKET_DIR/scripts/iptables.sh" ipv6-apply >> "$SOCKROCKET_LOG" 2>&1 \
                || log_warn "IPv6 dual-stack pending; IPv4 proxy left in place"
        else
            "$SOCKROCKET_DIR/scripts/iptables.sh" ipv6-guard >> "$SOCKROCKET_LOG" 2>&1 \
                || log_warn "IPv6 compatibility guard pending"
        fi
    fi

    # After any fail-open, try to put the proxy path back once healthy.
    try_recover_lan_proxy

    [ "$issues" -gt 0 ] && log_warn "Watchdog: $issues issue(s) found and addressed"
}

do_api_restart() { do_api_stop && do_api_start; }
do_network_restore() { restore_direct_network || return 1; ok "Direct network restored"; }

# Rebuild transparent proxy and DNS hijack for a new ipv6_mode while the
# proxy process keeps running. A process exit is reserved for crashes: the
# supervisor would otherwise fail-open the LAN and restart in a loop.
do_reapply_policy() {
    if [ -f "$SOCKROCKET_DIR/stopped" ] || ! is_running; then
        return 0
    fi
    # Fail-open owns the LAN until try_recover_lan_proxy has verified the
    # outbound. The saved policy is applied by that recovery.
    if [ -f "$DEGRADED_FILE" ]; then
        log_warn "Network policy saved; LAN stays direct until the proxy recovers"
        return 0
    fi
    local want_proxy want_dns
    want_proxy=$(conf_bool transparent_proxy true)
    want_dns=$(conf_bool dns_hijack false)
    # Touch only the IPv6 chains. iptables restart would remove the working
    # IPv4 proxy before the new policy is installed.
    if [ "$want_proxy" = "true" ] && ! wait_for_tun 5; then
        log_error "IPv6 policy reapply skipped; TUN is not ready. IPv4 rules left in place"
        return 1
    fi
    # Also runs without transparent proxy: it removes IPv6 DNS redirects to a
    # listener that only exists in TUN mode.
    if ! "$SOCKROCKET_DIR/scripts/iptables.sh" ipv6-apply >> "$SOCKROCKET_LOG" 2>&1; then
        log_error "IPv6 policy reapply failed; IPv4 rules left in place"
        return 1
    fi
    if [ "$want_dns" = "true" ]; then
        if ! iptables -t nat -C PREROUTING -j SOCKROCKET_DNS 2>/dev/null; then
            if ! "$SOCKROCKET_DIR/scripts/iptables.sh" dns-start >> "$SOCKROCKET_LOG" 2>&1; then
                enter_lan_degraded "IPv4 DNS hijack missing and could not be installed"
                return 1
            fi
        fi
    fi
    ok "Network policy reapplied without restarting the proxy"
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
# Ensure lock is released on script exit (even on signal).
# INT/TERM must exit after the handler — the EXIT trap then releases the
# lock. Trapping them without exiting would let a killed start/stop run on.
trap 'release_lock' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

case "$1" in
    boot)
        run_locked 15 do_boot
        ;;
    start)
        run_locked 15 do_start
        ;;
    stop)
        run_locked 15 do_stop
        ;;
    restart)
        run_locked 15 do_restart
        ;;
    reload)
        run_locked 15 do_restart
        ;;
    status)  do_status ;;
    software-center-status) do_softcenter_status ;;
    log)     tail -100 "$SOCKROCKET_LOG" 2>/dev/null || info "No log file found" ;;
    api-start)
        run_locked 5 do_api_start
        ;;
    api-stop)
        run_locked 5 do_api_stop
        ;;
    api-restart)
        run_locked 5 do_api_restart
        ;;
    update-subs)
        do_update_subs
        ;;
    watchdog) acquire_lock 5 && { do_watchdog; release_lock; } || log_warn "Watchdog: previous run still active, skipping" ;;
    ensure-tun)
        run_locked 5 ensure_tun
        ;;
    dns-on)
        run_locked 15 do_dns_on
        ;;
    dns-off)
        run_locked 15 do_dns_off
        ;;
    pin-dns)
        run_locked 5 do_pin_dns_event
        ;;
    dhcpv6-restore)
        run_locked 5 do_dhcpv6_restore
        ;;
    wan-restore)
        run_locked 5 do_wan_restore
        ;;
    proxy-off)
        run_locked 15 do_proxy_off
        ;;
    ensure-sshd)
        ensure_sshd
        ;;
    network-restore)
        # Emergency / uninstall helper: tear down proxy+DNS without needing a
        # running daemon. Guarantees direct LAN connectivity.
        run_locked 15 do_network_restore
        ;;
    reapply-policy)
        run_locked 15 do_reapply_policy
        ;;
    recover)
        # Second-scale re-arm after fail-open (called by the guard loop).
        run_locked 5 try_recover_lan_proxy
        ;;
    firewall-restore)
        run_locked 5 do_firewall_restore
        ;;
    nat-restore)
        run_locked 5 do_nat_restore
        ;;
    kill-half-dead)
        # Called by the guard subshell (cannot inherit functions). Optional
        # reason is $2; does not write the manual-stop marker.
        kill_half_dead_daemon "${2:-half-dead}"
        ;;
    diagnose)  cmd_diagnose ;;
    *)
        echo "Usage: $0 {boot|start|stop|restart|reload|status|software-center-status|log|diagnose|update-subs|watchdog|api-start|api-stop|api-restart|ensure-tun|ensure-sshd|dns-on|dns-off|pin-dns|proxy-off|network-restore|reapply-policy|recover|firewall-restore|nat-restore|kill-half-dead}"
        exit 1
        ;;
esac
