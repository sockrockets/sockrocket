#!/bin/sh
# Sockrocket iptables transparent proxy rules for AsusWRT-Merlin
#
# TCP mode : REDIRECT → HTTP proxy port
# UDP mode : ip route → TUN device (when TUN device exists)
# DNS mode : redirect LAN port 53 → Sockrocket DNS port
#
# Usage: iptables.sh {start|stop|restart|status|verify}

SOCKROCKET_DIR="/jffs/addons/sockrocket"
SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"

# Service environments (API bridge, init hooks) may have a minimal PATH
# without /usr/sbin — ipset/iptables live there. Pin it explicitly.
export PATH="/usr/sbin:/usr/bin:/sbin:/bin:$PATH"

CHAIN="SOCKROCKET_PREROUTING"
DNS_CHAIN="SOCKROCKET_DNS"
MANGLE_CHAIN="SOCKROCKET_MANGLE"
TUN_MARK=0x4765   # 'Ve' in hex
# Policy-routing table for fwmark 0x4765. NOT table 100: on AsusWRT the
# rt_tables file aliases table 100 → "wan0" (and 200 → wan1 for dual-WAN
# failover), and firmware WAN events rewrite/flush that table — silently
# destroying our `default dev sockrocket-tun` route (and conversely, our stop-path
# `ip route flush table 100` would wipe the firmware's WAN routes). 5370
# is unused firmware space ("Sockrocket" roughly spelled on a phone keypad).
SOCKROCKET_RT_TABLE=5370

# Kernel ipset holding the curated China IPv4 ranges (sockrocket-cli
# --dump-cn-cidrs). When the `cn_ipset_direct` config toggle is on, LAN
# packets whose destination is in this set RETURN from SOCKROCKET_MANGLE before
# the MARK rules — domestic traffic keeps hardware NAT and never enters
# the userspace TUN stack. Trade-off: domain-based user rules no longer
# apply to destinations that resolve into the CN ranges (TUN never sees
# those flows), which is why this is an opt-in toggle.
IPSET_NAME="sockrocket_cn"

# Read port from config dynamically (called at runtime, not parse time)
get_conf() { grep "^${1}:" "$SOCKROCKET_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'; }

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; NC='\033[0m'
info()  { logger -t sockrocket-iptables -p daemon.info "$*"; printf "${CYAN}[Sockrocket-iptables] %s${NC}\n" "$*"; }
ok()    { logger -t sockrocket-iptables -p daemon.info "$*"; printf "${GREEN}[Sockrocket-iptables] ✓ %s${NC}\n" "$*"; }
err()   { logger -t sockrocket-iptables -p daemon.err "$*"; printf "${RED}[Sockrocket-iptables] ✗ %s${NC}\n" "$*" >&2; }

# LAN subnet from nvram. NOTE: `nvram get <key>` exits 0 even when the key
# is unset and prints an empty string, so an `|| echo default` fallback does
# NOT catch the empty case — an empty value would compute LAN_SUBNET as
# 0.0.0.0/0 and mark ALL forwarded traffic into the TUN. Validate instead.
_valid_ip4() {
    case "$1" in *.*.*.*) ;; *) return 1 ;; esac
    case "$1" in *[!0-9.]*|*..*|.*|*.) return 1 ;; esac
    return 0
}
ROUTER_IP=$(nvram get lan_ipaddr 2>/dev/null)
LAN_NETMASK=$(nvram get lan_netmask 2>/dev/null)
_valid_ip4 "$ROUTER_IP"   || ROUTER_IP="192.168.1.1"
_valid_ip4 "$LAN_NETMASK" || LAN_NETMASK="255.255.255.0"

_ip2int() { echo "$1" | awk -F. '{print ($1*16777216)+($2*65536)+($3*256)+$4}'; }
_int2cidr() {
    # Octet-wise CIDR conversion — safe on 32-bit shells. The old shift loop
    # (`bit=2147483648; bit>>=1`) overflows to a negative number in busybox
    # ash and never reaches 0, spinning forever at script parse time.
    local IFS=.
    set -- $1
    local cidr=0 o
    for o in "$1" "$2" "$3" "$4"; do
        case "$o" in
            255) cidr=$((cidr+8)) ;;
            254) cidr=$((cidr+7)) ;;
            252) cidr=$((cidr+6)) ;;
            248) cidr=$((cidr+5)) ;;
            240) cidr=$((cidr+4)) ;;
            224) cidr=$((cidr+3)) ;;
            192) cidr=$((cidr+2)) ;;
            128) cidr=$((cidr+1)) ;;
            0)   ;;
            *)   echo 24; return ;;  # non-contiguous mask: sane default
        esac
    done
    echo "$cidr"
}
_network() {
    local ip_int mask_int net a b c d
    ip_int=$(_ip2int "$1"); mask_int=$(_ip2int "$2")
    net=$((ip_int & mask_int))
    a=$(( (net>>24)&255 )); b=$(( (net>>16)&255 ))
    c=$(( (net>>8)&255  )); d=$(( net&255 ))
    echo "$a.$b.$c.$d"
}
LAN_SUBNET="$(_network "$ROUTER_IP" "$LAN_NETMASK")/$(_int2cidr "$LAN_NETMASK")"

detect_tun_dev() {
    # Only ever the device sockrocket-cli itself creates. A generic "tun[0-9]" probe
    # would happily match an OpenVPN/other-VPN device and then route the whole
    # LAN into it — silently breaking connectivity through someone else's
    # tunnel. If Sockrocket's own device is absent, report failure and let the caller
    # skip transparent proxying.
    if ip link show sockrocket-tun >/dev/null 2>&1; then
        echo "sockrocket-tun"
        return 0
    fi
    # Fallback for older builds that used the kernel-assigned tun0 name, but
    # only when that device carries Sockrocket's gateway address — an OpenVPN device
    # would not.
    if ip -4 addr show tun0 2>/dev/null | grep -q "10.10.0.2"; then
        echo "tun0"
        return 0
    fi
    return 1
}

# Load (or reload) the CN ipset and insert the bypass rule into SOCKROCKET_MANGLE
# just before the first MARK rule. Idempotent: safe to call standalone
# (`ipset-on`) while the chain is live, or from do_start on a fresh chain.
do_ipset_start() {
    # NOTE: probe ipset by invoking it — this router's minimal busybox sh
    # (as spawned by the API bridge) lacks the `command` builtin, so the
    # usual `command -v ipset` guard fails with 127 even when ipset exists.
    ipset list >/dev/null 2>&1 || { err "ipset not available"; return 1; }
    ipset create "$IPSET_NAME" hash:net maxelem 4096 -exist 2>/dev/null || true
    # One atomic restore instead of ~800 individual `ipset add` forks.
    if ! "$SOCKROCKET_DIR/sockrocket-cli" --dump-cn-cidrs 2>/dev/null \
        | awk '{print "add '"$IPSET_NAME"' " $1}' \
        | ipset restore -exist 2>/dev/null; then
        err "failed to populate $IPSET_NAME"
        return 1
    fi
    if ! iptables -t mangle -C "$MANGLE_CHAIN" -m set --match-set "$IPSET_NAME" dst -j RETURN 2>/dev/null; then
        local mark_line
        mark_line=$(iptables -t mangle -L "$MANGLE_CHAIN" --line-numbers -n 2>/dev/null \
            | awk '/MARK/{print $1; exit}')
        if [ -n "$mark_line" ]; then
            iptables -t mangle -I "$MANGLE_CHAIN" "$mark_line" \
                -m set --match-set "$IPSET_NAME" dst -j RETURN
        else
            iptables -t mangle -A "$MANGLE_CHAIN" \
                -m set --match-set "$IPSET_NAME" dst -j RETURN
        fi
    fi
    ok "CN ipset direct applied ($IPSET_NAME, $(ipset list "$IPSET_NAME" 2>/dev/null | grep -c '^[0-9]') ranges)"
}

do_ipset_stop() {
    iptables -t mangle -D "$MANGLE_CHAIN" -m set --match-set "$IPSET_NAME" dst -j RETURN 2>/dev/null || true
    ipset destroy "$IPSET_NAME" 2>/dev/null || true
    ok "CN ipset direct removed"
}

flush_chains() {
    # Flush proxy chains only. NEVER flush SOCKROCKET_DNS here: firewall-start
    # (and every `iptables.sh start`) would wipe LAN :53 REDIRECT and never put
    # it back — devices hard-coded to 8.8.8.8 then get GFW-poisoned answers
    # until a full Sockrocket restart. DNS NAT is managed solely by dns-start /
    # dns-stop / do_stop.
    iptables -t nat -D PREROUTING -j SOCKROCKET_PREROUTING 2>/dev/null || true
    iptables -t nat -F SOCKROCKET_PREROUTING 2>/dev/null || true
    iptables -t nat -X SOCKROCKET_PREROUTING 2>/dev/null || true
    iptables -t mangle -D PREROUTING -j SOCKROCKET_MANGLE 2>/dev/null || true
    iptables -t mangle -F SOCKROCKET_MANGLE 2>/dev/null || true
    iptables -t mangle -X SOCKROCKET_MANGLE 2>/dev/null || true
    info "Flushed existing Sockrocket proxy iptables chains"
}

do_start() {
    # No-op cleanly when TUN is absent (firewall-start after reboot before the
    # daemon is up). Do not flush anything in that case — preserves DNS NAT.
    TUN_DEV=$(detect_tun_dev)
    if [ -z "$TUN_DEV" ]; then
        err "TUN device not found — transparent proxy NOT enabled (start sockrocket-cli with TUN first)"
        return 1
    fi
    flush_chains

    # Strict reverse-path filtering (firmware default: conf/default/rp_filter=1,
    # inherited by sockrocket-tun at creation) drops every packet sockrocket-cli injects into
    # the TUN: the packet's source is a public internet IP whose main-table
    # route points out the WAN, not the TUN, so the kernel discards it and the
    # whole LAN sees ERR_CONNECTION_TIMED_OUT with the proxy otherwise healthy.
    # Loose mode (2) accepts sources reachable via ANY interface — exactly the
    # TUN reinjection case. Effective rp_filter is max(all, iface), so fix
    # conf/all too when it is strict.
    # NOTE: write /proc directly — minimal busybox builds (Merlin) have no
    # `sysctl` applet, so `sysctl -w ... || true` fails SILENTLY and leaves
    # strict filtering in place (this exact bug blackholed the LAN once).
    # sockrocket-cli also relaxes rp_filter itself right after creating the device;
    # this is belt-and-braces for older binaries.
    if [ -w "/proc/sys/net/ipv4/conf/$TUN_DEV/rp_filter" ]; then
        echo 2 > "/proc/sys/net/ipv4/conf/$TUN_DEV/rp_filter" 2>/dev/null || true
    fi
    if [ "$(cat /proc/sys/net/ipv4/conf/all/rp_filter 2>/dev/null)" = "1" ]; then
        echo 2 > /proc/sys/net/ipv4/conf/all/rp_filter 2>/dev/null || true
    fi

    info "Setting up transparent proxy rules (fwmark → $TUN_DEV)..."
    info "LAN: $LAN_SUBNET  mark=$TUN_MARK"

    iptables -t mangle -N "$MANGLE_CHAIN" 2>/dev/null \
        || iptables -t mangle -F "$MANGLE_CHAIN"
    iptables -t mangle -A "$MANGLE_CHAIN" -d "$ROUTER_IP"    -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 10.0.0.0/8     -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 172.16.0.0/12  -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 192.168.0.0/16 -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 100.64.0.0/10  -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 127.0.0.0/8    -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 169.254.0.0/16 -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 224.0.0.0/4    -j RETURN
    iptables -t mangle -A "$MANGLE_CHAIN" -d 240.0.0.0/4    -j RETURN
    # QUIC/HTTP3 (UDP 443) must NOT enter the TUN: Sockrocket relays UDP payloads
    # over a TCP-oriented outbound (incorrect semantics), so captured QUIC
    # just stalls until the browser's fallback timer fires. Leaving it direct
    # keeps CN sites fast (their QUIC works) and costs nothing for blocked
    # sites — their QUIC is unreachable directly anyway, and the browser
    # falls back to TCP 443, which IS proxied below.
    iptables -t mangle -A "$MANGLE_CHAIN" -s "$LAN_SUBNET" -p udp --dport 443 -j RETURN
    # Optional: CN-range bypass BEFORE the MARK rules (hardware NAT for
    # domestic traffic). Gated by the cn_ipset_direct config toggle.
    if [ "$(get_conf cn_ipset_direct)" = "true" ]; then
        do_ipset_start
    fi
    # Mark all other LAN-originated TCP+UDP; policy routing sends them into TUN.
    iptables -t mangle -A "$MANGLE_CHAIN" -s "$LAN_SUBNET" -p tcp \
        -j MARK --set-mark "$TUN_MARK"
    iptables -t mangle -A "$MANGLE_CHAIN" -s "$LAN_SUBNET" -p udp \
        -j MARK --set-mark "$TUN_MARK"
    iptables -t mangle -C PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null \
        || iptables -t mangle -A PREROUTING -j "$MANGLE_CHAIN"

    ip rule  add fwmark "$TUN_MARK" table "$SOCKROCKET_RT_TABLE" 2>/dev/null || true
    ip route replace default dev "$TUN_DEV" table "$SOCKROCKET_RT_TABLE" 2>/dev/null || true

    # Return-path forwarding: replies sockrocket-cli injects into the TUN (src =
    # public internet IP, dst = LAN client) traverse the filter FORWARD
    # chain, whose Merlin default ends in DROP after a `DROP state INVALID`
    # rule. They normally pass via the ESTABLISHED accept — but conntrack
    # entries expire while the userspace ipstack keeps long-lived sessions
    # (its own timeout is longer), and every reply after expiry is INVALID
    # and dropped: the flow hangs mid-transfer with no RST either way.
    # Accept everything out of the TUN explicitly; the daemon only injects
    # packets for flows it accepted, so this grants no new reachability.
    iptables -C FORWARD -i "$TUN_DEV" -j ACCEPT 2>/dev/null \
        || iptables -I FORWARD 1 -i "$TUN_DEV" -j ACCEPT

    ok "Transparent proxy applied (LAN TCP+UDP → $TUN_DEV)"
}

# DNS hijack: force ALL LAN DNS queries (any destination — many devices and
# DHCP setups hard-code public resolvers like 8.8.8.8, whose UDP answers the
# GFW poisons on the wire) into the router's own dnsmasq, which does the
# china-split via Sockrocket's DNS listener. Without this, only clients that
# voluntarily query the router get clean answers.
# Router-originated queries traverse OUTPUT, not PREROUTING, so dnsmasq's own
# upstreams (direct server + Sockrocket listener) are never looped back.
do_dns_start() {
    iptables -t nat -N "$DNS_CHAIN" 2>/dev/null \
        || iptables -t nat -F "$DNS_CHAIN"
    # Queries already addressed to the router land in dnsmasq directly.
    iptables -t nat -A "$DNS_CHAIN" -d "$ROUTER_IP" -j RETURN
    iptables -t nat -A "$DNS_CHAIN" -s "$LAN_SUBNET" -p udp --dport 53 \
        -j REDIRECT --to-ports 53
    iptables -t nat -A "$DNS_CHAIN" -s "$LAN_SUBNET" -p tcp --dport 53 \
        -j REDIRECT --to-ports 53
    iptables -t nat -C PREROUTING -j "$DNS_CHAIN" 2>/dev/null \
        || iptables -t nat -A PREROUTING -j "$DNS_CHAIN"
    ok "DNS hijack applied (LAN :53 → router dnsmasq)"
}

do_dns_stop() {
    iptables -t nat -D PREROUTING -j "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -F "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -X "$DNS_CHAIN" 2>/dev/null || true
}

do_stop() {
    info "Removing transparent proxy rules..."

    # TCP rules
    iptables -t nat -D PREROUTING -j "$CHAIN"     2>/dev/null || true
    iptables -t nat -F "$CHAIN" 2>/dev/null || true
    iptables -t nat -X "$CHAIN" 2>/dev/null || true

    # DNS rules
    do_dns_stop

    # UDP/mangle rules
    iptables -t mangle -D PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -F "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -X "$MANGLE_CHAIN" 2>/dev/null || true

    # TUN return-path forward accept (see do_start). Also clear the legacy
    # tun0 rule older builds may have installed.
    iptables -D FORWARD -i sockrocket-tun -j ACCEPT 2>/dev/null || true
    iptables -D FORWARD -i tun0 -j ACCEPT 2>/dev/null || true

    # CN ipset bypass (may be absent — destroy is idempotent)
    do_ipset_stop 2>/dev/null || true

    # ip rule / route for our private table only.
    #
    # NEVER `ip route flush table 100`: on AsusWRT table 100 is aliased to
    # wan0 (dual-WAN uses 200 → wan1). Flushing it kills the firmware's WAN
    # default route. Older Sockrocket builds may still have left an ip *rule*
    # pointing at table 100 — delete that rule alone and leave wan0 routes
    # untouched.
    ip rule  del fwmark "$TUN_MARK" table "$SOCKROCKET_RT_TABLE" 2>/dev/null || true
    ip route flush table "$SOCKROCKET_RT_TABLE" 2>/dev/null || true
    ip rule  del fwmark "$TUN_MARK" table 100 2>/dev/null || true

    ok "iptables rules removed"
}

do_status() {
    echo "=== $CHAIN (nat) ==="
    iptables -t nat    -L "$CHAIN"     -n --line-numbers 2>/dev/null || echo "(not active)"
    echo ""
    echo "=== $DNS_CHAIN (nat) ==="
    iptables -t nat    -L "$DNS_CHAIN" -n --line-numbers 2>/dev/null || echo "(not active)"
    echo ""
    echo "=== $MANGLE_CHAIN (mangle) ==="
    iptables -t mangle -L "$MANGLE_CHAIN" -n --line-numbers 2>/dev/null || echo "(not active)"
}

do_verify() {
    local errors=0
    iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "MARK" || {
        err "SOCKROCKET_MANGLE chain missing or has no MARK rules"
        errors=$((errors + 1))
    }
    iptables -t mangle -C PREROUTING -j SOCKROCKET_MANGLE 2>/dev/null || {
        err "SOCKROCKET_MANGLE jump not in PREROUTING chain"
        errors=$((errors + 1))
    }
    ip rule 2>/dev/null | grep -q "fwmark $TUN_MARK" || {
        err "ip rule for fwmark $TUN_MARK missing"
        errors=$((errors + 1))
    }
    if [ "$(get_conf cn_ipset_direct)" = "true" ]; then
        iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "match-set $IPSET_NAME" || {
            err "cn_ipset_direct enabled but $IPSET_NAME bypass rule missing"
            errors=$((errors + 1))
        }
    fi
    [ "$errors" -eq 0 ] && ok "iptables rules verified OK" && return 0
    err "iptables verification FAILED ($errors issue(s))"
    return 1
}

case "$1" in
    start)   do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_start ;;
    dns-start) do_dns_start ;;
    dns-stop)  do_dns_stop ;;
    ipset-on)  do_ipset_start ;;
    ipset-off) do_ipset_stop ;;
    status)  do_status ;;
    verify)  do_verify ;;
    *)
        echo "Usage: $0 {start|stop|restart|dns-start|dns-stop|ipset-on|ipset-off|status|verify}"
        exit 1
        ;;
esac
