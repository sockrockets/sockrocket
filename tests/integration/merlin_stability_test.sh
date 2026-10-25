#!/bin/sh
# Focused fault-injection tests; no router, real firewall, DNS, or network.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
SOURCE="$ROOT/merlin/scripts/sockrocket.sh"
PASS=0

check() { [ "$1" = "$2" ] || { echo "FAIL: $3 (expected '$1', got '$2')" >&2; exit 1; }; }

fixture() {
    SANDBOX=$(mktemp -d)
    # Load actual functions, without executing the command dispatcher.
    sed '/^case "\$1" in/,$d' "$SOURCE" > "$SANDBOX/functions.sh"
    . "$SANDBOX/functions.sh"
    SOCKROCKET_DIR="$SANDBOX/plugin"
    SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
    SOCKROCKET_BIN="$SOCKROCKET_DIR/sockrocket-cli"
    SOCKROCKET_LOG="$SOCKROCKET_DIR/sockrocket.log"
    SOCKROCKET_CRASH_LOG="$SOCKROCKET_DIR/crash-history.log"
    SOCKROCKET_PID="$SOCKROCKET_DIR/sockrocket.pid"
    DEGRADED_FILE="$SOCKROCKET_DIR/.lan_degraded"
    WOUT_FAILS_FILE="$SOCKROCKET_DIR/.wout_fails"
    LOCK_DIR="$SOCKROCKET_DIR/.lockdir"
    SOCKROCKET_DNSMASQ_DIR="$SANDBOX/dnsmasq.d"
    SOCKROCKET_RESOLV_FILE="$SANDBOX/resolv.dnsmasq"
    SOCKROCKET_MEMINFO_FILE="$SANDBOX/meminfo"
    export SOCKROCKET_DNSMASQ_DIR SOCKROCKET_RESOLV_FILE SOCKROCKET_MEMINFO_FILE
    mkdir -p "$SOCKROCKET_DIR/scripts" "$SOCKROCKET_DNSMASQ_DIR"
    trap 'release_lock; rm -rf "$SANDBOX"' EXIT
    printf 'MemTotal: 100000 kB\nMemAvailable: 80000 kB\n' > "$SOCKROCKET_MEMINFO_FILE"
    printf 'server=192.0.2.53\n' > "$SOCKROCKET_RESOLV_FILE"
    cat > "$SOCKROCKET_CONF" <<'YAML'
mode: "tun"
transparent_proxy: true
dns_hijack: true
dns_port: 5300
nodes:
  - name: "test"
YAML
    printf 'server=127.0.0.1#__DNS_PORT__\n' > "$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"
    printf '#!/bin/sh\nexit 0\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    chmod +x "$SOCKROCKET_DIR/scripts/iptables.sh"
    printf '#!/bin/sh\nexit 0\n' > "$SOCKROCKET_BIN"
    chmod +x "$SOCKROCKET_BIN"
    logger() { :; }
    nvram() { case "$2" in ipv6_service) echo disabled ;; lan_ifname) echo br0 ;; lan_netmask) echo 255.255.255.0 ;; *) echo 192.168.1.1 ;; esac; }
    pidof() { :; }
    netstat() { echo 'udp 0 0 127.0.0.1:5300 0.0.0.0:*'; }
    service() { echo "$*" >> "$SANDBOX/service.calls"; }
    is_running() { return 0; }
    outbound_probe_ok() { return 0; }
    ensure_sshd() { :; }
    sleep() { :; }
}

run_test() {
    name="$1"
    if ( fixture; "$name" ); then
        PASS=$((PASS+1)); echo "PASS: $name"
    else
        echo "FAIL: $name" >&2; exit 1
    fi
}

test_dns_failure_rolls_back() {
    service() {
        echo "$*" >> "$SANDBOX/service.calls"
        [ "$(wc -l < "$SANDBOX/service.calls" | tr -d ' ')" -gt 1 ]
    }
    if update_dnsmasq start; then echo 'restart failure was hidden' >&2; exit 1; fi
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'snippet rolled back'
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'direct upstream kept'
    check 2 "$(wc -l < "$SANDBOX/service.calls" | tr -d ' ')" 'direct dnsmasq restarted after failure'
}

test_repeated_cleanup_does_not_restart_dns() {
    update_dnsmasq start
    update_dnsmasq stop
    before=$(wc -l < "$SANDBOX/service.calls" | tr -d ' ')
    update_dnsmasq stop
    update_dnsmasq stop
    check "$before" "$(wc -l < "$SANDBOX/service.calls" | tr -d ' ')" 'idle cleanup is idempotent'
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'upstream restored'
}

test_merlin_3006_main_dns_isolated_from_sdn() {
    # RT-AX86U Pro firmware may omit the mktemp command entirely.
    mktemp() { echo 'unexpected mktemp call' >&2; return 127; }
    SOCKROCKET_DNS_UPSTREAM_FILE="$SANDBOX/main-upstream"
    SOCKROCKET_DNS_MAIN_CONFIG="$SANDBOX/dnsmasq.conf"
    SOCKROCKET_DNS_POSTCONF_HOOK="$SANDBOX/dnsmasq.postconf"
    SOCKROCKET_DNS_POSTCONF_SCRIPT="$SANDBOX/dnsmasq-postconf.sh"
    export SOCKROCKET_DNS_UPSTREAM_FILE SOCKROCKET_DNS_MAIN_CONFIG
    cp "$ROOT/merlin/scripts/dnsmasq-postconf.sh" "$SOCKROCKET_DNS_POSTCONF_SCRIPT"
    chmod +x "$SOCKROCKET_DNS_POSTCONF_SCRIPT"
    printf 'dnsmasq-postconf.sh\n' > "$SOCKROCKET_DNS_POSTCONF_HOOK"
    apply_postconf=1
    nvram() { case "$2" in firmver) echo 3.0.0.6 ;; lan_ipaddr) echo 192.168.50.1 ;; esac; }
    service() {
        echo "$*" >> "$SANDBOX/service.calls"
        printf 'servers-file=/tmp/resolv.dnsmasq\n' > "$SOCKROCKET_DNS_MAIN_CONFIG"
        if [ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] &&
           [ -f "$SOCKROCKET_DNS_UPSTREAM_FILE" ] &&
           [ -f "$SOCKROCKET_DNS_POSTCONF_HOOK" ] &&
           [ "$apply_postconf" = 1 ]; then
            "$SOCKROCKET_DNS_POSTCONF_SCRIPT" "$SOCKROCKET_DNS_MAIN_CONFIG"
        fi
    }
    update_dnsmasq start
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'shared SDN upstream preserved'
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_DNS_UPSTREAM_FILE")" 'main LAN has private upstream'
    check "servers-file=$SOCKROCKET_DNS_UPSTREAM_FILE" "$(cat "$SOCKROCKET_DNS_MAIN_CONFIG")" 'main dnsmasq uses private upstream'
    update_dnsmasq stop
    check absent "$([ -f "$SOCKROCKET_DNS_UPSTREAM_FILE" ] && echo present || echo absent)" 'private upstream removed on stop'
    check 'servers-file=/tmp/resolv.dnsmasq' "$(cat "$SOCKROCKET_DNS_MAIN_CONFIG")" 'main dnsmasq restored on stop'
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'SDN upstream unchanged on stop'
    # A binary-only update without the new postconf hook must fail open.
    rm "$SOCKROCKET_DNS_POSTCONF_HOOK"
    if update_dnsmasq start; then echo '3006 DNS activated without postconf hook' >&2; exit 1; fi
    check absent "$([ -f "$SOCKROCKET_DNS_UPSTREAM_FILE" ] && echo present || echo absent)" 'no orphan private upstream'
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'SDN upstream preserved on failed start'
    printf 'dnsmasq-postconf.sh\n' > "$SOCKROCKET_DNS_POSTCONF_HOOK"
    apply_postconf=0
    # rc returned without calling the hook. The control script applies the
    # same edit itself, so a skipped callback does not drop LAN DNS.
    SOCKROCKET_DNS_ISOLATION_WAIT=0
    update_dnsmasq start
    check "servers-file=$SOCKROCKET_DNS_UPSTREAM_FILE" "$(grep '^servers-file=' "$SOCKROCKET_DNS_MAIN_CONFIG")" 'skipped hook still isolates main upstream'
    update_dnsmasq stop
    # A helper that cannot rewrite the config still fails open.
    printf '#!/bin/sh\nexit 1\n' > "$SANDBOX/fail-postconf.sh"
    chmod +x "$SANDBOX/fail-postconf.sh"
    SOCKROCKET_DNS_POSTCONF_SCRIPT="$SANDBOX/fail-postconf.sh"
    if update_dnsmasq start; then echo '3006 DNS activated when isolation edit failed' >&2; exit 1; fi
    check absent "$([ -f "$SOCKROCKET_DNS_UPSTREAM_FILE" ] && echo present || echo absent)" 'failed isolation rolls back private upstream'
    SOCKROCKET_DNS_POSTCONF_SCRIPT="$SANDBOX/dnsmasq-postconf.sh"
    apply_postconf=1
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_DNS_UPSTREAM_FILE"
    printf 'servers-file=/tmp/resolv.dnsmasq\nserver=1.1.1.1\n' > "$SANDBOX/dnsmasq-1.conf"
    "$SOCKROCKET_DNS_POSTCONF_SCRIPT" "$SANDBOX/dnsmasq-1.conf"
    check 'servers-file=/tmp/resolv.dnsmasq
server=1.1.1.1' "$(cat "$SANDBOX/dnsmasq-1.conf")" 'SDN dnsmasq config untouched'
    printf '%s\n' \
        'servers-file=/tmp/resolv.dnsmasq ' \
        'server=1.1.1.1' \
        'server=127.0.0.1#5453' \
        'server=/router.asus.com/192.168.50.1' \
        'address=/use-application-dns.net/' \
        > "$SOCKROCKET_DNS_MAIN_CONFIG"
    "$SOCKROCKET_DNS_POSTCONF_SCRIPT" "$SOCKROCKET_DNS_MAIN_CONFIG"
    check "servers-file=$SOCKROCKET_DNS_UPSTREAM_FILE" "$(grep '^servers-file=' "$SOCKROCKET_DNS_MAIN_CONFIG")" 'trailing space still isolates main upstream'
    check 0 "$(grep -c '^server=1\.1\.1\.1$' "$SOCKROCKET_DNS_MAIN_CONFIG" || true)" 'ISP server= cannot outrun Sockrocket'
    check 0 "$(grep -c '^server=127\.0\.0\.1#5453$' "$SOCKROCKET_DNS_MAIN_CONFIG" || true)" 'Stubby server= cannot outrun Sockrocket'
    check 'server=/router.asus.com/192.168.50.1' "$(grep '^server=/' "$SOCKROCKET_DNS_MAIN_CONFIG")" 'domain DNS override kept'
    check 'address=/use-application-dns.net/' "$(grep '^address=' "$SOCKROCKET_DNS_MAIN_CONFIG")" 'address rule kept'
    printf '%s\n' 'servers-file=/tmp/fork-resolv.dnsmasq' 'server=9.9.9.9' > "$SOCKROCKET_DNS_MAIN_CONFIG"
    "$SOCKROCKET_DNS_POSTCONF_SCRIPT" "$SOCKROCKET_DNS_MAIN_CONFIG"
    check "servers-file=$SOCKROCKET_DNS_UPSTREAM_FILE" "$(grep '^servers-file=' "$SOCKROCKET_DNS_MAIN_CONFIG")" 'non-stock main servers-file still isolates'
    check 0 "$(grep -c '^server=9\.9\.9\.9$' "$SOCKROCKET_DNS_MAIN_CONFIG" || true)" 'fork upstream server= cannot outrun Sockrocket'
    # An upgrade must restore the old shared-file pin before starting SDNs.
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_RESOLV_FILE"
    printf 'server=192.0.2.53\n' > "$SOCKROCKET_RESOLV_FILE.Sockrocket-bak"
    printf 'dnsmasq-postconf.sh\n' > "$SOCKROCKET_DNS_POSTCONF_HOOK"
    update_dnsmasq start
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'legacy shared pin migrated'
    printf '4242\n' > "$SANDBOX/main.pid"
    SOCKROCKET_DNS_MAIN_PID="$SANDBOX/main.pid"
    export SOCKROCKET_DNS_MAIN_PID
    : > "$SANDBOX/hup"
    kill() { echo "$*" >> "$SANDBOX/hup"; }
    chmod 600 "$SOCKROCKET_DNS_UPSTREAM_FILE"
    pin_dns_upstream
    check '-HUP 4242' "$(cat "$SANDBOX/hup")" 'unreadable private upstream reloads main dnsmasq'
    case "$(ls -l "$SOCKROCKET_DNS_UPSTREAM_FILE")" in
        -rw-r--r--*) ;;
        *) echo 'private upstream stayed unreadable' >&2; exit 1 ;;
    esac
    : > "$SANDBOX/hup"
    pin_dns_upstream
    check empty "$(test -s "$SANDBOX/hup" && echo nonempty || echo empty)" 'readable private upstream is left running'
}

test_lost_listener_unpins_dns() {
    update_dnsmasq start
    netstat() { :; }
    printf '#!/bin/sh\nexit 1\n' > "$SOCKROCKET_BIN"
    if update_dnsmasq start; then exit 1; fi
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'failed activation removes dead DNS pin'
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'no dead listener config'
}

test_missing_tun_never_recovers() {
    echo fault > "$DEGRADED_FILE"
    wait_for_tun() { return 1; }
    if try_recover_lan_proxy; then exit 1; fi
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'degrade marker retained'
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'DNS not installed without TUN'
}

test_firewall_failure_never_recovers() {
    echo fault > "$DEGRADED_FILE"
    wait_for_tun() { return 0; }
    printf '#!/bin/sh\n[ "$1" != start ]\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    if try_recover_lan_proxy; then exit 1; fi
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'firewall failure retains marker'
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'no fake-IP DNS without routing'
}

test_memory_pressure_delays_recovery() {
    echo fault > "$DEGRADED_FILE"
    printf 'MemTotal: 100000 kB\nMemAvailable: 10000 kB\n' > "$SOCKROCKET_MEMINFO_FILE"
    wait_for_tun() { return 0; }
    if try_recover_lan_proxy; then exit 1; fi
    check 90 "$(memory_used_pct)" 'high memory pressure measured'
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'guard cannot immediately re-arm'
    printf 'MemTotal: 100000 kB\nMemAvailable: 22000 kB\n' > "$SOCKROCKET_MEMINFO_FILE"
    try_recover_lan_proxy
    check absent "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'recovery below threshold'
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'recovery pins DNS before clearing degraded marker'
}

test_old_kernel_memory_fallback() {
    printf 'MemTotal: 100000 kB\nMemFree: 10000 kB\nBuffers: 5000 kB\nCached: 15000 kB\n' > "$SOCKROCKET_MEMINFO_FILE"
    check 70 "$(memory_used_pct)" 'missing MemAvailable uses free + buffers + cache'
}

test_log_rotation_preserves_writer() {
    LOG_MAX_BYTES=64
    printf '%0100d\n' 0 > "$SOCKROCKET_LOG"
    exec 3>> "$SOCKROCKET_LOG"
    rotate_log
    printf 'AFTER_ROTATION\n' >&3
    exec 3>&-
    grep -q AFTER_ROTATION "$SOCKROCKET_LOG" || { echo 'open writer lost on rotation' >&2; exit 1; }
    size=$(wc -c < "$SOCKROCKET_LOG" | tr -d ' ')
    [ "$size" -lt 100 ] || exit 1
}

test_degraded_start_still_creates_tun() {
    echo fault > "$DEGRADED_FILE"
    printf '#!/bin/sh\nexit 0\n' > "$SOCKROCKET_BIN"; chmod +x "$SOCKROCKET_BIN"
    running=0
    is_running() { [ "$running" = 1 ]; }
    do_api_start() { :; }
    kill_stray_daemons() { :; }
    enable_core_dumps() { :; }
    ensure_tun() { return 0; }
    setup_cron() { :; }
    start_guard() { :; }
    start_daemon_supervised() { echo "$1" > "$SANDBOX/launch-tun"; running=1; echo 99999 > "$SOCKROCKET_PID"; }
    wait_for_tun() { return 1; }
    do_start || true
    check 1 "$(cat "$SANDBOX/launch-tun")" 'recovery-capable daemon launched with TUN'
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'no premature success'
}

test_command_failure_exit_status() {
    rm -f "$SOCKROCKET_BIN"
    # Run actual dispatch, not just the function, against a missing binary.
    sed "s#^SOCKROCKET_DIR=\"/jffs/addons/sockrocket\"#SOCKROCKET_DIR=\"$SOCKROCKET_DIR\"#" "$SOURCE" > "$SANDBOX/dispatch.sh"
    if sh "$SANDBOX/dispatch.sh" start >/dev/null 2>&1; then echo 'start failure returned success' >&2; exit 1; fi
    check absent "$([ -d "$LOCK_DIR" ] && echo present || echo absent)" 'failed action releases lock'
}

test_nat_restore_reinstalls_dns_without_undoing_fail_open() {
    printf '#!/bin/sh\necho "$1" >> "$SANDBOX/firewall.calls"\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    export SANDBOX
    wait_for_dns() { return 0; }
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    echo fault > "$DEGRADED_FILE"
    do_nat_restore
    check absent "$([ -f "$SANDBOX/firewall.calls" ] && echo present || echo absent)" 'NAT rebuild cannot undo fail-open'
    rm -f "$DEGRADED_FILE"
    do_nat_restore
    check dns-start "$(head -1 "$SANDBOX/firewall.calls")" 'NAT rebuild restores DNS hijack'
    printf '\nipv6_mode: dual_stack\n' >> "$SOCKROCKET_CONF"
    : > "$SANDBOX/firewall.calls"
    do_nat_restore
    check dns-start "$(head -1 "$SANDBOX/firewall.calls")" 'dual-stack NAT rebuild still restores IPv4 DNS first'
    check ipv6-apply "$(tail -1 "$SANDBOX/firewall.calls")" 'dual-stack NAT rebuild restores IPv6 DNS'
}

test_firewall_event_respects_degraded_and_stopped() {
    printf '#!/bin/sh\necho "$1" >> "$SANDBOX/firewall.calls"\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    export SANDBOX
    wait_for_tun() { return 0; }
    echo fault > "$DEGRADED_FILE"
    do_firewall_restore
    check absent "$([ -f "$SANDBOX/firewall.calls" ] && echo present || echo absent)" 'WAN/firewall event cannot undo fail-open'
    rm -f "$DEGRADED_FILE"; touch "$SOCKROCKET_DIR/stopped"
    do_firewall_restore
    check absent "$([ -f "$SANDBOX/firewall.calls" ] && echo present || echo absent)" 'manual stop survives firewall rebuild'
}

test_wan_reconnect_restores_rules_and_dns_safely() {
    printf '#!/bin/sh\necho "$1" >> "$SANDBOX/firewall.calls"\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    export SANDBOX
    wait_for_tun() { return 0; }
    wait_for_dns() { return 0; }
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    do_wan_restore
    check start "$(head -1 "$SANDBOX/firewall.calls")" 'WAN reconnect restores transparent routing'
    check dns-start "$(tail -1 "$SANDBOX/firewall.calls")" 'WAN reconnect restores DNS interception'
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'WAN reconnect re-pins DNS'

    printf 'server=198.51.100.53\n' > "$SOCKROCKET_RESOLV_FILE"
    pin_dns_upstream
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'DHCP renew re-pins DNS'
    check 'server=198.51.100.53' "$(cat "$SOCKROCKET_RESOLV_FILE.Sockrocket-bak")" 'fresh WAN DNS saved for Stop'

    : > "$SANDBOX/firewall.calls"
    printf 'server=198.51.100.54\n' > "$SOCKROCKET_RESOLV_FILE"
    touch "$SOCKROCKET_DIR/stopped"
    do_wan_restore
    pin_dns_upstream
    check empty "$(test -s "$SANDBOX/firewall.calls" && echo nonempty || echo empty)" 'manual Stop keeps WAN rules down'
    check 'server=198.51.100.54' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'manual Stop keeps firmware DNS'

    rm -f "$SOCKROCKET_DIR/stopped"
    echo fault > "$DEGRADED_FILE"
    do_wan_restore
    do_pin_dns_event
    check empty "$(test -s "$SANDBOX/firewall.calls" && echo nonempty || echo empty)" 'fail-open keeps WAN rules down'
    check 'server=198.51.100.54' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'fail-open keeps firmware DNS'

    rm -f "$DEGRADED_FILE"
    sed -i.bak 's/^dns_hijack: true/dns_hijack: false/' "$SOCKROCKET_CONF"
    pin_dns_upstream
    check 'server=198.51.100.54' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'disabled DNS hijack never re-pins'
}

test_dhcpv6_prefix_change_reconciles_ipv6_without_ipv4_rebuild() {
    printf '#!/bin/sh\necho "$1" >> "$SANDBOX/firewall.calls"\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    export SANDBOX
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    printf '\nipv6_mode: dual_stack\n' >> "$SOCKROCKET_CONF"
    do_dhcpv6_restore
    check ipv6-apply "$(cat "$SANDBOX/firewall.calls")" 'DHCPv6 event reconciles only IPv6 policy'
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'DHCPv6 event re-pins DNS'

    : > "$SANDBOX/firewall.calls"
    pin_dns_upstream() { return 1; }
    if do_dhcpv6_restore; then echo 'DNS pin failure was hidden' >&2; exit 1; fi
    check ipv6-apply "$(cat "$SANDBOX/firewall.calls")" 'DNS failure does not block IPv6 policy repair'

    : > "$SANDBOX/firewall.calls"
    printf 'server=198.51.100.53\n' > "$SOCKROCKET_RESOLV_FILE"
    touch "$SOCKROCKET_DIR/stopped"
    do_dhcpv6_restore
    check empty "$(test -s "$SANDBOX/firewall.calls" && echo nonempty || echo empty)" 'manual Stop blocks DHCPv6 restore'
    check 'server=198.51.100.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'manual Stop keeps firmware DNS'

    rm -f "$SOCKROCKET_DIR/stopped"
    echo fault > "$DEGRADED_FILE"
    do_dhcpv6_restore
    check empty "$(test -s "$SANDBOX/firewall.calls" && echo nonempty || echo empty)" 'fail-open blocks DHCPv6 restore'
    check 'server=198.51.100.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'fail-open keeps firmware DNS'
}

test_reapply_respects_fail_open_and_clears_stale_ipv6_dns() {
    printf '#!/bin/sh\necho "$1" >> "$SANDBOX/firewall.calls"\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    export SANDBOX
    wait_for_tun() { return 0; }
    iptables() { return 0; }
    echo fault > "$DEGRADED_FILE"
    do_reapply_policy
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'saving settings keeps fail-open armed'
    check absent "$([ -f "$SANDBOX/firewall.calls" ] && echo present || echo absent)" 'fail-open LAN is not re-captured by a save'
    rm -f "$DEGRADED_FILE"
    sed -i.bak 's/^transparent_proxy: true/transparent_proxy: false/' "$SOCKROCKET_CONF"
    do_reapply_policy
    check ipv6-apply "$(head -1 "$SANDBOX/firewall.calls")" 'non-transparent save still removes IPv6 DNS redirects'
}

test_policy_routing_staged_and_idempotent() {
    sed '/^case "\$1" in/,$d' "$ROOT/merlin/scripts/iptables.sh" > "$SANDBOX/iptables-functions.sh"
    . "$SANDBOX/iptables-functions.sh"
    SOCKROCKET_DIR="$SANDBOX/plugin"
    SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
    detect_tun_dev() { echo sockrocket-tun; }
    iptables() {
        echo "iptables $*" >> "$SANDBOX/kernel.calls"
        case " $* " in *' -C '*) return 1 ;; esac
        return 0
    }
    fail_route=0
    fail_fake_route=0
    ip() {
        echo "ip $*" >> "$SANDBOX/kernel.calls"
        case "$*" in
            'route replace default '*) [ "$fail_route" = 0 ] ;;
            'route replace 198.18.0.0/16 dev '*) [ "$fail_fake_route" = 0 ] ;;
            'rule show') [ ! -f "$SANDBOX/rule-installed" ] || echo '32765: from all fwmark 0x4765 lookup 5370' ;;
            'rule add '*) touch "$SANDBOX/rule-installed" ;;
            *) return 0 ;;
        esac
    }
    do_start
    do_start
    check 2 "$(grep -c -- '-A SOCKROCKET_MANGLE -i br0 -s 192.168.1.0/24 -p tcp' "$SANDBOX/kernel.calls")" 'capture stays on the primary LAN bridge'
    check 1 "$(grep -c '^ip rule add ' "$SANDBOX/kernel.calls")" 'policy rule not duplicated on reapply'
    check 2 "$(grep -c '^ip route replace 198.18.0.0/16 dev sockrocket-tun$' "$SANDBOX/kernel.calls")" 'Fake-IP reply reverse route installed'
    check 0 "$(grep -c '^ip route replace unreachable 198.18.0.0/16' "$SANDBOX/kernel.calls" || true)" 'active TUN never rejects Fake-IP source route'
    route_line=$(awk '/^ip route replace default/{print NR;exit}' "$SANDBOX/kernel.calls")
    attach_line=$(awk '/^iptables -t mangle -A PREROUTING/{print NR;exit}' "$SANDBOX/kernel.calls")
    [ "$route_line" -lt "$attach_line" ] || { echo 'MARK exposed before route' >&2; exit 1; }
    fail_route=1
    if do_start; then echo 'route failure hidden' >&2; exit 1; fi
    check 2 "$(grep -c '^iptables -t mangle -A PREROUTING' "$SANDBOX/kernel.calls")" 'failed route never attaches MARK chain'
    fail_route=0
    fail_fake_route=1
    if do_start; then echo 'Fake-IP reverse route failure hidden' >&2; exit 1; fi
    check 2 "$(grep -c '^iptables -t mangle -A PREROUTING' "$SANDBOX/kernel.calls")" 'failed Fake-IP reverse route never attaches MARK chain'
    fail_fake_route=0
    tun_inserts=0
    iptables() {
        echo "iptables $*" >> "$SANDBOX/kernel.calls"
        case " $* " in
            *' -C '*) return 1 ;;
            *' -I FORWARD 1 -i sockrocket-tun '*)
                tun_inserts=$((tun_inserts + 1))
                [ "$tun_inserts" -lt 2 ] || return 1 ;;
        esac
        return 0
    }
    if do_start; then echo 'final return-path failure hidden' >&2; exit 1; fi
    check 2 "$(grep -c '^iptables -t mangle -A PREROUTING' "$SANDBOX/kernel.calls")" 'failed final ACCEPT never attaches MARK chain'
    iptables() {
        echo "iptables $*" >> "$SANDBOX/kernel.calls"
        case " $* " in *' -C '*) return 1 ;; esac
        return 0
    }
    : > "$SANDBOX/kernel.calls"
    printf '#!/bin/sh\nexit 0\n' > "$SOCKROCKET_DIR/sockrocket-cli"
    chmod +x "$SOCKROCKET_DIR/sockrocket-cli"
    do_start
    check 0 "$(grep -c -- '--dport 443 -j RETURN' "$SANDBOX/kernel.calls" || true)" 'QUIC stays inside the proxy for a UDP node'
    check 0 "$(grep -c -- '-I FORWARD 1 -s .*198.18.*-p udp' "$SANDBOX/kernel.calls" || true)" 'native fake-IP UDP is not rejected'
    : > "$SANDBOX/kernel.calls"
    printf '#!/bin/sh\n[ "$1" = --dump-rule-sources ] && exit 0\nexit 1\n' > "$SOCKROCKET_DIR/sockrocket-cli"
    printf '\nproxy_mode: global\n' >> "$SOCKROCKET_CONF"
    do_start
    check 0 "$(grep -c -- '--dport 443 -j RETURN' "$SANDBOX/kernel.calls" || true)" 'global mode with stream-only node keeps real-address QUIC in userspace policy'
    check 0 "$(grep -c -- '-I FORWARD 1 -s .*198.18.*-p udp' "$SANDBOX/kernel.calls" || true)" 'stream-only fake-IP UDP reaches userspace'
    # Device policies must reach userspace before the optional CN bypass.
    nvram() { case "$2" in ipv6_service) echo disabled ;; lan_ifname) echo br0 ;; lan_netmask) echo 255.255.255.0 ;; *) echo 192.0.2.1 ;; esac; }
    : > "$SANDBOX/kernel.calls"
    printf '#!/bin/sh\ncase "$1" in --dump-rule-sources) printf "192.0.2.10/32\\n192.0.2.0/24\\n";; *) exit 1;; esac\n' > "$SOCKROCKET_DIR/sockrocket-cli"
    do_ipset_start() { iptables -t mangle -A "$MANGLE_CHAIN" -m set --match-set sockrocket_cn dst -j RETURN; }
    printf '\ncn_ipset_direct: true\n' >> "$SOCKROCKET_CONF"
    do_start
    check 4 "$(grep -c '^-*iptables -t mangle -A SOCKROCKET_DEVICES -s ' "$SANDBOX/kernel.calls")" 'both TCP and UDP device scopes marked'
    check 0 "$(grep -c '^iptables -A SOCKROCKET_UDP_GUARD ' "$SANDBOX/kernel.calls" || true)" 'QUIC is not a LAN-wide kernel switch'
    device_line=$(awk '/-A SOCKROCKET_MANGLE -i br0 -s .* -j SOCKROCKET_DEVICES/{print NR;exit}' "$SANDBOX/kernel.calls")
    cn_line=$(awk '/--match-set sockrocket_cn dst/{print NR;exit}' "$SANDBOX/kernel.calls")
    [ -n "$device_line" ] && [ -n "$cn_line" ] && [ "$device_line" -lt "$cn_line" ] || { echo 'device policies bypassed' >&2;exit 1; }
    check 0 "$(grep -c -- '--dport 443 -j RETURN' "$SANDBOX/kernel.calls" || true)" 'real-address QUIC cannot bypass device or global policy'
    flush_chains
    check 2 "$(grep -c -- '-X SOCKROCKET_DEVICES' "$SANDBOX/kernel.calls")" 'device chain removed on rebuild and explicit cleanup'
    : > "$SANDBOX/kernel.calls"
    printf '#!/bin/sh\nexit 1\n' > "$SOCKROCKET_DIR/sockrocket-cli"
    if do_start; then echo 'invalid source export hidden' >&2;exit 1;fi
    check 0 "$(grep -c -- '-A PREROUTING -j SOCKROCKET_MANGLE' "$SANDBOX/kernel.calls" || true)" 'source validation failure never attaches LAN marks'

}

test_proxy_endpoints_leave_directly() {
    sed '/^case "\$1" in/,$d' "$ROOT/merlin/scripts/iptables.sh" > "$SANDBOX/iptables-functions.sh"
    . "$SANDBOX/iptables-functions.sh"
    SOCKROCKET_DIR="$SANDBOX/plugin"
    SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
    detect_tun_dev() { echo sockrocket-tun; }
    iptables() {
        echo "iptables $*" >> "$SANDBOX/kernel.calls"
        case " $* " in *' -C '*) return 1 ;; esac
        return 0
    }
    ip6tables() {
        echo "ip6tables $*" >> "$SANDBOX/kernel.calls"
        case " $* " in *' -C '*) return 1 ;; esac
        return 0
    }
    ip() { echo "ip $*" >> "$SANDBOX/kernel.calls"; return 0; }
    printf '#!/bin/sh\ncase "$1" in --dump-proxy-endpoints) printf "203.0.113.10\\n198.18.0.5\\n10.1.1.1\\n2001:db8::10\\nfe80::1\\nnot-an-ip\\n";; esac\nexit 0\n' \
        > "$SOCKROCKET_DIR/sockrocket-cli"
    chmod +x "$SOCKROCKET_DIR/sockrocket-cli"
    : > "$SANDBOX/kernel.calls"
    do_start
    check 1 "$(grep -c -- '-A SOCKROCKET_MANGLE -d 203.0.113.10 -j RETURN' "$SANDBOX/kernel.calls")" 'node address leaves directly'
    check 0 "$(grep -c -- '-d 198.18.0.5 -j RETURN' "$SANDBOX/kernel.calls" || true)" 'Fake-IP is not a direct endpoint'
    check 0 "$(grep -c -- '-d 10.1.1.1 -j RETURN' "$SANDBOX/kernel.calls" || true)" 'private node address is not added twice'
    check 0 "$(grep -c -- 'not-an-ip' "$SANDBOX/kernel.calls" || true)" 'endpoint dump cannot inject iptables arguments'
    endpoint_line=$(awk '/-d 203.0.113.10 -j RETURN/{print NR; exit}' "$SANDBOX/kernel.calls")
    mark_line=$(awk '/-A SOCKROCKET_MANGLE -i br0 .* -j MARK/{print NR; exit}' "$SANDBOX/kernel.calls")
    [ -n "$endpoint_line" ] && [ -n "$mark_line" ] && [ "$endpoint_line" -lt "$mark_line" ] \
        || { echo 'node bypass is after the LAN mark' >&2; exit 1; }
}

test_primary_lan_scope_validation() {
    sed '/^case "\$1" in/,$d' "$ROOT/merlin/scripts/iptables.sh" > "$SANDBOX/iptables-functions.sh"
    . "$SANDBOX/iptables-functions.sh"
    lan_ip=10.42.7.1 lan_mask=255.255.255.128 lan_bridge=br5
    nvram() { case "$2" in lan_ipaddr) echo "$lan_ip" ;; lan_netmask) echo "$lan_mask" ;; lan_ifname) echo "$lan_bridge" ;; ipv6_service) echo disabled ;; esac; }
    load_lan_scope
    check 10.42.7.0/25 "$LAN_SUBNET" 'non-default LAN subnet computed'
    check br5 "$LAN_IFACE" 'LAN bridge taken from firmware'
    iptables() { echo "iptables $*" >> "$SANDBOX/invalid-kernel.calls"; return 0; }
    ip6tables() { echo "ip6tables $*" >> "$SANDBOX/invalid-kernel.calls"; return 1; }
    ip() { echo "ip $*" >> "$SANDBOX/invalid-kernel.calls"; return 0; }
    detect_tun_dev() { echo sockrocket-tun; }
    for bad in '999.1.1.1|255.255.255.0' '010.42.7.1|255.255.255.0' \
        '10.42.7.1|255.0.255.0' '10.42.7.1|255.255.0.255' \
        '10.42.7.1|0.0.0.0' '|255.255.255.0' '10.42.7.1|'; do
        lan_ip=${bad%%|*}; lan_mask=${bad#*|}
        if load_lan_scope; then echo "accepted invalid LAN: $bad" >&2; exit 1; fi
        if do_start; then echo "started with invalid LAN: $bad" >&2; exit 1; fi
        if do_dns_start; then echo "DNS captured invalid LAN: $bad" >&2; exit 1; fi
    done
    lan_ip=10.42.7.1 lan_mask=255.255.255.0 lan_bridge='br0 -j ACCEPT'
    if load_lan_scope; then echo 'accepted invalid LAN interface' >&2; exit 1; fi
    check absent "$([ -f "$SANDBOX/invalid-kernel.calls" ] && echo present || echo absent)" 'invalid LAN cannot change firewall'
    # Stop must still remove owned rules when NVRAM becomes unreadable.
    do_stop
    grep -q 'iptables -t mangle -D PREROUTING -j SOCKROCKET_MANGLE' "$SANDBOX/invalid-kernel.calls"
    if grep -q 'iptables -D FORWARD -s  ' "$SANDBOX/invalid-kernel.calls"; then
        echo 'cleanup used an empty legacy source' >&2; exit 1
    fi
}

test_startup_failure_never_enables_dns() {
    want_proxy=true; want_dns=true
    wait_for_tun() { return 1; }
    if activate_startup_network; then exit 1; fi
    check present "$([ -f "$DEGRADED_FILE" ] && echo present || echo absent)" 'startup failure records fail-open'
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'no fake-IP DNS without TUN'
    wait_for_tun() { return 0; }
    printf '#!/bin/sh\n[ "$1" != start ]\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    if activate_startup_network; then exit 1; fi
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'no fake-IP DNS after firewall failure'
}

test_dns_nat_failure_rolls_back() {
    printf '#!/bin/sh\n[ "$1" != dns-start ]\n' > "$SOCKROCKET_DIR/scripts/iptables.sh"
    if update_dnsmasq start; then exit 1; fi
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'NAT failure restores upstream'
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'NAT failure removes snippet'
}

test_dns_nat_rule_failure_is_reported() {
    sed '/^case "\$1" in/,$d' "$ROOT/merlin/scripts/iptables.sh" > "$SANDBOX/nat-functions.sh"
    . "$SANDBOX/nat-functions.sh"
    iptables() {
        echo "$*" >> "$SANDBOX/nat.calls"
        case "$*" in *'--dport 53'*) return 1 ;; esac
        return 0
    }
    if do_dns_start; then echo 'NAT rule failure hidden' >&2; exit 1; fi
    check 1 "$(grep -c -- '-D PREROUTING' "$SANDBOX/nat.calls")" 'partial DNS NAT detached'
    check 0 "$(grep -c -- '-A PREROUTING' "$SANDBOX/nat.calls" || true)" 'failed chain never exposed'
}

test_dns_answer_failure_blocks_activation() {
    printf '#!/bin/sh\nexit 1\n' > "$SOCKROCKET_BIN"
    if update_dnsmasq start; then echo 'unresponsive DNS was activated' >&2; exit 1; fi
    check absent "$([ -f "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf" ] && echo present || echo absent)" 'listening but failing DNS is not installed'
}
test_subscription_refresh_preserves_stop() {
    : > "$SOCKROCKET_DIR/stopped"
    do_start() { echo 'unexpected start' >&2; return 1; }
    do_stop() { echo 'unexpected stop' >&2; return 1; }
    cat > "$SOCKROCKET_BIN" <<'MOCK'
#!/bin/sh
[ "$QUERY_STRING" = action=update_subs ] || exit 9
[ "$REQUEST_METHOD" = POST ] || exit 9
printf 'Content-Type: application/json\nCache-Control: no-cache\n\n{"ok":true}\n'
MOCK
    chmod +x "$SOCKROCKET_BIN"
    do_update_subs
    check present "$([ -f "$SOCKROCKET_DIR/stopped" ] && echo present || echo absent)" 'cron update preserves manual stop'
    printf '#!/bin/sh\nexit 7\n' > "$SOCKROCKET_BIN"
    if do_update_subs; then echo 'failed subscription update hidden' >&2; exit 1; fi
    check present "$([ -f "$SOCKROCKET_DIR/stopped" ] && echo present || echo absent)" 'failed update preserves stop'
    check 0 "$(ls "$SOCKROCKET_DIR"/subscription-update.*.tmp 2>/dev/null | wc -l | tr -d ' ')" 'private response temps removed'
}
test_dns_restore_failure_keeps_backup() {
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_RESOLV_FILE"
    printf 'server=192.0.2.53\n' > "${SOCKROCKET_RESOLV_FILE}.Sockrocket-bak"
    cp() { return 1; }
    if unpin_dns_upstream; then echo 'restore copy failure hidden' >&2; exit 1; fi
    check 'server=127.0.0.1#5300' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'failed restore does not truncate current file'
    check 'server=192.0.2.53' "$(cat "${SOCKROCKET_RESOLV_FILE}.Sockrocket-bak")" 'failed restore keeps backup'
}
test_dns_backup_failure_does_not_pin() {
    cp() { return 1; }
    cpfile="$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    printf 'server=127.0.0.1#5300\n' > "$cpfile"
    if pin_dns_upstream; then echo 'backup copy failure hidden' >&2; exit 1; fi
    check 'server=192.0.2.53' "$(cat "$SOCKROCKET_RESOLV_FILE")" 'backup failure leaves direct upstream unchanged'
}
test_dns_runtime_readable_backup_private() {
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    pin_dns_upstream
    check '-rw-r--r--' "$(ls -l "$SOCKROCKET_RESOLV_FILE" | awk '{print substr($1,1,10)}')" 'dnsmasq can read pinned upstream after privilege drop'
    check '-rw-------' "$(ls -l "${SOCKROCKET_RESOLV_FILE}.Sockrocket-bak" | awk '{print substr($1,1,10)}')" 'upstream backup remains private'
    unpin_dns_upstream
    check '-rw-r--r--' "$(ls -l "$SOCKROCKET_RESOLV_FILE" | awk '{print substr($1,1,10)}')" 'restored upstream readable by dnsmasq'
    chmod 600 "$SOCKROCKET_RESOLV_FILE"
    update_dnsmasq stop
    check '-rw-r--r--' "$(ls -l "$SOCKROCKET_RESOLV_FILE" | awk '{print substr($1,1,10)}')" 'legacy private upstream healed on stop'
}
test_fault_and_user_stop_preserve_mapping() {
    ip() { :; }
    printf 'synthetic mapping\n' > "$SOCKROCKET_DIR/fakeip.store"
    restore_direct_network
    check present "$([ -f "$SOCKROCKET_DIR/fakeip.store" ] && echo present || echo absent)" 'fault recovery preserves mappings for cached LAN answers'
    : > "$SOCKROCKET_DIR/stopped"
    restore_direct_network
    check present "$([ -f "$SOCKROCKET_DIR/fakeip.store" ] && echo present || echo absent)" 'explicit stop preserves cached LAN mapping for restart'
}
test_existing_dns_pin_permissions_healed() {
    : > "$SOCKROCKET_DNSMASQ_DIR/sockrocket.conf"
    printf 'server=127.0.0.1#5300\n' > "$SOCKROCKET_RESOLV_FILE"
    chmod 600 "$SOCKROCKET_RESOLV_FILE"
    pidof() { echo 999999; }
    kill() { echo "$*" >> "$SANDBOX/signals"; }
    pin_dns_upstream 5300
    pin_dns_upstream 5300
    check 1 "$(wc -l < "$SANDBOX/signals" | tr -d ' ')" 'healed permissions reload upstream exactly once' 
    check '-rw-r--r--' "$(ls -l "$SOCKROCKET_RESOLV_FILE" | awk '{print substr($1,1,10)}')" 'matching DNS pin is readable after privilege drop'
}
test_private_log_permissions_healed() {
    : > "$SOCKROCKET_LOG"
    : > "$SOCKROCKET_CRASH_LOG"
    chmod 666 "$SOCKROCKET_LOG" "$SOCKROCKET_CRASH_LOG"
    rotate_log
    for log in "$SOCKROCKET_LOG" "$SOCKROCKET_CRASH_LOG"; do
        check '-rw-------' "$(ls -l "$log" | awk '{print substr($1,1,10)}')" 'existing private log permissions healed'
    done
}
test_background_restart_preserves_manual_stop() {
    : > "$SOCKROCKET_DIR/stopped"
    do_stop() { echo stop >> "$SANDBOX/restart.calls"; }
    do_start() { echo start >> "$SANDBOX/restart.calls"; }
    do_restart
    check absent "$([ -f "$SANDBOX/restart.calls" ] && echo present || echo absent)" 'background restart preserves manual stop'
    rm "$SOCKROCKET_DIR/stopped"
    do_restart
    check 2 "$(wc -l < "$SANDBOX/restart.calls" | tr -d ' ')" 'running service restarts normally'
}
test_boot_keeps_fresh_install_and_manual_stop_disabled() {
    : > "$SOCKROCKET_DIR/stopped"
    do_api_start() { echo api >> "$SANDBOX/boot.calls"; }
    do_start() { echo proxy >> "$SANDBOX/boot.calls"; }
    do_boot
    check api "$(cat "$SANDBOX/boot.calls")" 'boot starts only management while stopped'
    check present "$([ -f "$SOCKROCKET_DIR/stopped" ] && echo present || echo absent)" 'boot preserves Stop'
    rm "$SOCKROCKET_DIR/stopped" "$SANDBOX/boot.calls"
    do_boot
    check 'api proxy' "$(tr '\n' ' ' < "$SANDBOX/boot.calls" | sed 's/ $//')" 'enabled boot resumes proxy'
    rm "$SANDBOX/boot.calls"
    do_api_start() { return 1; }
    do_boot
    check proxy "$(cat "$SANDBOX/boot.calls")" 'management API failure does not suppress enabled proxy boot'
}

test_softcenter_state_tracks_proxy_not_api_or_saved_intent() {
    which() { [ "$1" = dbus ] || command -v "$1"; }
    dbus() {
        case "$1" in
            get) cat "$SANDBOX/$2" 2>/dev/null ;;
            set) printf '%s' "${2#*=}" > "$SANDBOX/${2%%=*}"; echo write >> "$SANDBOX/dbus.calls" ;;
        esac
    }
    local running=0
    is_running() { [ "$running" = 1 ]; }
    echo 123 > "$SOCKROCKET_PID"
    echo 456 > "$SOCKROCKET_DIR/sockrocket-api.pid"
    sync_softcenter_state
    check 0 "$(dbus get sockrocket_enable)" 'API-only process is disabled'
    check 'Sockrocket stopped' "$(do_softcenter_status)" 'stopped report must not contain API PID'
    running=1
    sync_softcenter_state
    check 1 "$(dbus get sockrocket_enable)" 'actual proxy is enabled'
    check 1 "$(dbus get softcenter_module_sockrocket_enable)" 'tile metadata matches proxy'
    check 'Sockrocket running (PID 123)' "$(do_softcenter_status)" 'runtime probe identifies only proxy'
    local writes
    writes=$(wc -l < "$SANDBOX/dbus.calls" | tr -d ' ')
    sync_softcenter_state
    check "$writes" "$(wc -l < "$SANDBOX/dbus.calls" | tr -d ' ')" 'unchanged polls do not write flash'
    : > "$SOCKROCKET_DIR/stopped"
    sync_softcenter_state
    check 0 "$(dbus get sockrocket_enable)" 'stop intent wins while daemon is draining'
    rm "$SOCKROCKET_DIR/stopped"
    running=0
    sync_softcenter_state
    check 0 "$(dbus get softcenter_module_sockrocket_enable)" 'crashed daemon is not enabled'
}

run_test test_boot_keeps_fresh_install_and_manual_stop_disabled
run_test test_primary_lan_scope_validation
run_test test_softcenter_state_tracks_proxy_not_api_or_saved_intent
run_test test_background_restart_preserves_manual_stop
run_test test_existing_dns_pin_permissions_healed
run_test test_private_log_permissions_healed
run_test test_fault_and_user_stop_preserve_mapping
run_test test_dns_runtime_readable_backup_private
run_test test_dns_restore_failure_keeps_backup
run_test test_dns_backup_failure_does_not_pin
run_test test_subscription_refresh_preserves_stop
run_test test_dns_answer_failure_blocks_activation
run_test test_dns_nat_rule_failure_is_reported
run_test test_startup_failure_never_enables_dns
run_test test_dns_nat_failure_rolls_back
run_test test_dns_failure_rolls_back
run_test test_repeated_cleanup_does_not_restart_dns
run_test test_merlin_3006_main_dns_isolated_from_sdn
run_test test_lost_listener_unpins_dns
run_test test_missing_tun_never_recovers
run_test test_firewall_failure_never_recovers
run_test test_memory_pressure_delays_recovery
run_test test_old_kernel_memory_fallback
run_test test_log_rotation_preserves_writer
run_test test_degraded_start_still_creates_tun
run_test test_command_failure_exit_status
run_test test_nat_restore_reinstalls_dns_without_undoing_fail_open
run_test test_firewall_event_respects_degraded_and_stopped
run_test test_wan_reconnect_restores_rules_and_dns_safely
run_test test_dhcpv6_prefix_change_reconciles_ipv6_without_ipv4_rebuild
run_test test_reapply_respects_fail_open_and_clears_stale_ipv6_dns
run_test test_policy_routing_staged_and_idempotent
run_test test_proxy_endpoints_leave_directly
echo "$PASS stability scenarios passed"
