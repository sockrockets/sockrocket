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
IPV6_CHAIN="SOCKROCKET_IPV6_GUARD"
IPV6_MANGLE="SOCKROCKET_IPV6_MANGLE"
IPV6_FORWARD="SOCKROCKET_IPV6_FORWARD"
IPV6_DNS="SOCKROCKET_IPV6_DNS"
TUN_MARK=0x4765   # 'Ve' in hex
# Same ULA the daemon assigns in configure_tun_ipv6. A new interface often
# inherits disable_ipv6=1; that flag is per device, not a global IPv6 ban.
TUN_IPV6_ADDR=fd9a:4c2e:17b0::2/64
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
# the userspace TUN stack. Sources with device rules are marked before this
# bypass. Trade-off: global domain-based user rules no longer
# apply to destinations that resolve into the CN ranges (TUN never sees
# those flows), which is why this is an opt-in toggle.
IPSET_NAME="sockrocket_cn"

# Read port from config dynamically (called at runtime, not parse time)
get_conf() { grep "^${1}:" "$SOCKROCKET_CONF" 2>/dev/null | awk '{print $2}' | head -1 | tr -d '"'; }

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; NC='\033[0m'
info()  { logger -t sockrocket-iptables -p daemon.info "$*"; printf "${CYAN}[Sockrocket-iptables] %s${NC}\n" "$*"; }
ok()    { logger -t sockrocket-iptables -p daemon.info "$*"; printf "${GREEN}[Sockrocket-iptables] ✓ %s${NC}\n" "$*"; }
err()   { logger -t sockrocket-iptables -p daemon.err "$*"; printf "${RED}[Sockrocket-iptables] ✗ %s${NC}\n" "$*" >&2; }

# NVRAM can be temporarily empty during boot or LAN reconfiguration. Never
# substitute a guessed subnet: that could capture another network's traffic.
_valid_ip4() {
    awk -v addr="$1" 'BEGIN {
        if (split(addr, octet, ".") != 4) exit 1
        for (i = 1; i <= 4; i++) {
            if (octet[i] !~ /^[0-9]+$/ || length(octet[i]) > 3 ||
                (length(octet[i]) > 1 && substr(octet[i], 1, 1) == "0") ||
                octet[i] + 0 > 255) exit 1
        }
    }'
}

_ip2int() { echo "$1" | awk -F. '{print ($1*16777216)+($2*65536)+($3*256)+$4}'; }
_int2cidr() {
    # Octet-wise CIDR conversion — safe on 32-bit shells. The old shift loop
    # (`bit=2147483648; bit>>=1`) overflows to a negative number in busybox
    # ash and never reaches 0, spinning forever at script parse time.
    local IFS=.
    set -- $1
    local cidr=0 o seen_partial=0
    for o in "$1" "$2" "$3" "$4"; do
        # Once a mask octet is not 255, all following octets must be zero.
        [ "$seen_partial" = 0 ] || [ "$o" = 0 ] || return 1
        case "$o" in
            255) cidr=$((cidr+8)) ;;
            254) cidr=$((cidr+7)); seen_partial=1 ;;
            252) cidr=$((cidr+6)); seen_partial=1 ;;
            248) cidr=$((cidr+5)); seen_partial=1 ;;
            240) cidr=$((cidr+4)); seen_partial=1 ;;
            224) cidr=$((cidr+3)); seen_partial=1 ;;
            192) cidr=$((cidr+2)); seen_partial=1 ;;
            128) cidr=$((cidr+1)); seen_partial=1 ;;
            0)   seen_partial=1 ;;
            *)   return 1 ;;
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
load_lan_scope() {
    local prefix
    ROUTER_IP=$(nvram get lan_ipaddr 2>/dev/null)
    LAN_NETMASK=$(nvram get lan_netmask 2>/dev/null)
    LAN_IFACE=$(ipv6_lan_interface) || return 1
    _valid_ip4 "$ROUTER_IP" && _valid_ip4 "$LAN_NETMASK" || return 1
    prefix=$(_int2cidr "$LAN_NETMASK") || return 1
    # A /0 NVRAM value would match every forwarded IPv4 client.
    [ "$prefix" -gt 0 ] || return 1
    LAN_SUBNET="$(_network "$ROUTER_IP" "$LAN_NETMASK")/$prefix"
}

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
    # ALWAYS recreate. `ipset create -exist` keeps the previous maxelem; an
    # older 4096-cap set silently truncated the expanded CN list (~6k+) and
    # CN destinations fell through to MARK → proxy.
    iptables -t mangle -D "$MANGLE_CHAIN" -m set --match-set "$IPSET_NAME" dst -j RETURN 2>/dev/null || true
    ipset destroy "$IPSET_NAME" 2>/dev/null || true
    if ! ipset create "$IPSET_NAME" hash:net family inet hashsize 4096 maxelem 32768 2>/dev/null; then
        err "failed to create $IPSET_NAME"
        return 1
    fi
    # One atomic restore instead of thousands of individual `ipset add` forks.
    if ! "$SOCKROCKET_DIR/sockrocket-cli" --dump-cn-cidrs 2>/dev/null \
        | awk '{print "add '"$IPSET_NAME"' " $1}' \
        | ipset restore 2>/dev/null; then
        err "failed to populate $IPSET_NAME"
        ipset destroy "$IPSET_NAME" 2>/dev/null || true
        return 1
    fi
    local loaded want
    loaded=$(ipset list "$IPSET_NAME" 2>/dev/null | grep -c '^[0-9]')
    want=$("$SOCKROCKET_DIR/sockrocket-cli" --dump-cn-cidrs 2>/dev/null | wc -l)
    want=$(echo "$want" | tr -d ' ')
    if [ -n "$want" ] && [ "$want" -gt 0 ] && [ "$loaded" -lt "$want" ]; then
        err "CN ipset truncated ($loaded/$want) — check maxelem"
        return 1
    fi
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
    ok "CN ipset direct applied ($IPSET_NAME, $loaded ranges)"
}

do_ipset_stop() {
    iptables -t mangle -D "$MANGLE_CHAIN" -m set --match-set "$IPSET_NAME" dst -j RETURN 2>/dev/null || true
    ipset destroy "$IPSET_NAME" 2>/dev/null || true
    ok "CN ipset direct removed"
}

# Merlin transparent routing is IPv4-only. DNS AAAA suppression alone cannot
# prevent IPv6 literals, cached records or DoH from bypassing that routing.
# Reject forwarded LAN IPv6 promptly while proxying; applications can retry
# over IPv4. Router INPUT/OUTPUT and same-bridge LAN traffic stay untouched.
ipv6_guard_needed() {
    local mode
    mode=$(nvram get ipv6_service 2>/dev/null)
    case "$mode" in ""|disabled) ;; *) return 0 ;; esac
    ip -6 route show default 2>/dev/null | grep -q . && return 0
    ip -6 addr show scope global 2>/dev/null | grep -q 'inet6 '
}

ipv6_lan_interface() {
    local iface
    iface=$(nvram get lan_ifname 2>/dev/null)
    iface=${iface:-br0}
    case "$iface" in *[!a-zA-Z0-9_.:-]*) return 1 ;; esac
    printf '%s\n' "$iface"
}

# Runtime IPv6 state lives on tmpfs: it describes installed kernel rules,
# which a reboot clears. A persistent copy would outlive those rules.
SOCKROCKET_RUNTIME_DIR="${SOCKROCKET_RUNTIME_DIR:-/tmp/sockrocket-run}"
ipv6_state() {
    mkdir -p "$SOCKROCKET_RUNTIME_DIR" 2>/dev/null
    printf '%s/ipv6_%s' "$SOCKROCKET_RUNTIME_DIR" "$1"
}

ipv6_clear_fallback() {
    rm -f "$(ipv6_state fallback)"
}

# Remember a dual-stack miss without repeating the same line every minute.
ipv6_note_fallback() {
    local reason="$1" old
    old=$(cat "$(ipv6_state fallback)" 2>/dev/null || true)
    if [ "$old" != "$reason" ]; then
        printf '%s\n' "$reason" > "$(ipv6_state fallback)"
        err "$reason"
    fi
}

ipv6_compat_guard_stop() {
    local count=0
    while [ "$count" -lt 32 ] && ip6tables -D FORWARD -j "$IPV6_CHAIN" 2>/dev/null; do
        count=$((count + 1))
    done
    if ip6tables -C FORWARD -j "$IPV6_CHAIN" 2>/dev/null; then
        err "IPv6 protection cleanup failed; owned FORWARD jump remains"
        return 1
    fi
    ip6tables -F "$IPV6_CHAIN" 2>/dev/null || true
    ip6tables -X "$IPV6_CHAIN" 2>/dev/null || true
}

# Reject forwarded LAN IPv6 so clients fall back to the IPv4 proxy.
# This does not touch IPv4 rules.
ipv6_compat_guard_present() {
    local iface
    iface=$(ipv6_lan_interface) || return 1
    ip6tables -C "$IPV6_CHAIN" -i "$iface" ! -o "$iface" \
        -j REJECT --reject-with icmp6-adm-prohibited 2>/dev/null &&
    ipv6_forward_jump_ok "$IPV6_CHAIN"
}

ipv6_compat_guard_start() {
    ipv6_guard_needed || return 0
    local iface
    iface=$(ipv6_lan_interface) || { err "Invalid LAN interface for IPv6 protection"; return 1; }
    # Re-inserting an intact guard would open a window with no guard at all.
    ipv6_compat_guard_present && return 0
    ipv6_compat_guard_stop || return 1
    if ! {
        { ip6tables -N "$IPV6_CHAIN" 2>/dev/null || ip6tables -F "$IPV6_CHAIN"; } &&
        ip6tables -A "$IPV6_CHAIN" -i "$iface" ! -o "$iface" \
            -j REJECT --reject-with icmp6-adm-prohibited &&
        ip6tables -I FORWARD 1 -j "$IPV6_CHAIN"
    }; then
        ipv6_compat_guard_stop || true
        err "IPv6 bypass protection unavailable — transparent proxy NOT enabled"
        return 1
    fi
    ok "IPv4 proxy mode: external LAN IPv6 rejected for IPv4 fallback (restored on stop)"
}

ipv6_guard_stop() {
    ipv6_proxy_stop || return 1
    ipv6_compat_guard_stop || return 1
}

# Install dual-stack when this host can complete it. Otherwise keep the IPv4
# proxy and install the compatibility guard. A missing prefix, forwarding
# flag, or ip6tables target must not flush IPv4 rules.
ipv6_try_dual_stack() {
    if ipv6_proxy_verify; then
        ipv6_compat_guard_stop || return 1
        ipv6_clear_fallback
        return 0
    fi
    # Keep LAN IPv6 rejected while the dual-stack chain is rebuilt: the guard
    # is only removed once the new chain verifies above it.
    ipv6_compat_guard_start || return 1
    ipv6_proxy_stop || return 1
    if ipv6_proxy_start; then
        return 0
    fi
    ipv6_proxy_stop || return 1
    ipv6_note_fallback "IPv6 dual-stack unavailable; IPv4 proxy kept and external IPv6 rejected"
    ipv6_compat_guard_start
}

ipv6_guard_start() {
    if [ "$(get_conf ipv6_mode)" = "dual_stack" ]; then
        ipv6_try_dual_stack
        return $?
    fi
    ipv6_clear_fallback
    ipv6_compat_guard_start
}

ipv6_guard_verify() {
    if [ "$(get_conf ipv6_mode)" = "dual_stack" ]; then
        ipv6_proxy_verify
        return $?
    fi
    ipv6_guard_needed || return 0
    local iface first
    iface=$(ipv6_lan_interface) || return 1
    ip6tables -C "$IPV6_CHAIN" -i "$iface" ! -o "$iface" \
        -j REJECT --reject-with icmp6-adm-prohibited 2>/dev/null || return 1
    ipv6_forward_jump_ok "$IPV6_CHAIN"
}

# IPv6 has independent policy tables. Never change the WAN default route,
# firmware RA/DHCPv6 state, forwarding sysctls or the router's OUTPUT chain.
ipv6_proxy_stop() {
    local count=0
    while [ "$count" -lt 32 ] && ip6tables -t mangle -D PREROUTING -j "$IPV6_MANGLE" 2>/dev/null; do count=$((count+1)); done
    ip6tables -t mangle -C PREROUTING -j "$IPV6_MANGLE" 2>/dev/null && return 1
    ip6tables -t mangle -F "$IPV6_MANGLE" 2>/dev/null || true
    ip6tables -t mangle -X "$IPV6_MANGLE" 2>/dev/null || true
    ip6tables -t mangle -F SOCKROCKET_IPV6_DEVICES 2>/dev/null || true
    ip6tables -t mangle -X SOCKROCKET_IPV6_DEVICES 2>/dev/null || true
    count=0
    while [ "$count" -lt 32 ] && ip6tables -D FORWARD -j "$IPV6_FORWARD" 2>/dev/null; do count=$((count+1)); done
    ip6tables -C FORWARD -j "$IPV6_FORWARD" 2>/dev/null && return 1
    ip6tables -F "$IPV6_FORWARD" 2>/dev/null || true
    ip6tables -X "$IPV6_FORWARD" 2>/dev/null || true
    count=0
    while [ "$count" -lt 32 ] && ip -6 rule del fwmark "$TUN_MARK" table "$SOCKROCKET_RT_TABLE" 2>/dev/null; do count=$((count+1)); done
    ip -6 rule show 2>/dev/null | grep -Eq "fwmark ${TUN_MARK}(/0xffffffff)? .*lookup ${SOCKROCKET_RT_TABLE}([[:space:]]|$)" && return 1
    ip -6 route del default dev sockrocket-tun table "$SOCKROCKET_RT_TABLE" 2>/dev/null || true
    ip -6 route del 2001:2:0:4765::/96 dev sockrocket-tun 2>/dev/null || true
    return 0
}

# Enable IPv6 on the proxy TUN only and install its ULA. Forwarding and
# every other interface are left to the firmware.
ensure_tun_ipv6() {
    local tun="$1" sysctl
    sysctl="/proc/sys/net/ipv6/conf/$tun/disable_ipv6"
    if [ -w "$sysctl" ] && [ "$(cat "$sysctl" 2>/dev/null)" != 0 ]; then
        echo 0 > "$sysctl" 2>/dev/null || true
    fi
    ip -6 addr replace "$TUN_IPV6_ADDR" dev "$tun"
}

ipv6_proxy_start() {
    local iface tun dest proto router_addresses
    iface=$(ipv6_lan_interface) || return 1
    tun=$(detect_tun_dev) || return 1
    # ICMPv6 and forwarding belong to the firmware; enabling a disabled
    # forwarding sysctl here could disable acceptance of WAN advertisements.
    ensure_tun_ipv6 "$tun" || true
    if [ "$(cat /proc/sys/net/ipv6/conf/all/forwarding 2>/dev/null)" != 1 ] ||
       ! ip -6 addr show dev "$tun" 2>/dev/null | grep -q 'inet6 '; then
        err "IPv6 dual-stack needs firmware IPv6 forwarding and an IPv6 address on the TUN device"
        return 1
    fi
    if ! {
        { ip6tables -t mangle -N "$IPV6_MANGLE" 2>/dev/null || ip6tables -t mangle -F "$IPV6_MANGLE"; } &&
        { ip6tables -N "$IPV6_FORWARD" 2>/dev/null || ip6tables -F "$IPV6_FORWARD"; }
    }; then ipv6_proxy_stop; return 1; fi
    # Multicast/link-local/ULA traffic stays on the LAN or firmware path.
    for dest in ::/128 ::1/128 fc00::/7 fe80::/10 ff00::/8; do
        ip6tables -t mangle -A "$IPV6_MANGLE" -d "$dest" -j RETURN || { ipv6_proxy_stop; return 1; }
    done
    router_addresses=$(ip -6 addr show dev "$iface" 2>/dev/null | awk '/inet6 /{print $2}')
    for dest in $router_addresses; do
        # Keep on-link LAN traffic out of the transparent internet path.
        ip6tables -t mangle -A "$IPV6_MANGLE" -d "$dest" -j RETURN || { ipv6_proxy_stop; return 1; }
    done
    proxy_endpoint_returns 6 || { ipv6_proxy_stop; return 1; }
    # Device scopes are marked before other LAN policy so the userspace
    # router sees their source even when a kernel direct shortcut is enabled.
    local device_sources device_source device_proto
    device_sources=$("$SOCKROCKET_DIR/sockrocket-cli" --dump-rule-sources-v6 "$SOCKROCKET_CONF" 2>/dev/null) || {
        ipv6_proxy_stop
        err "Cannot validate IPv6 device rule sources"
        return 1
    }
    if [ -n "$device_sources" ]; then
        if ! ip6tables -t mangle -N SOCKROCKET_IPV6_DEVICES 2>/dev/null \
            && ! ip6tables -t mangle -F SOCKROCKET_IPV6_DEVICES; then
            ipv6_proxy_stop
            return 1
        fi
        for device_source in $device_sources; do
            for device_proto in tcp udp; do
                ip6tables -t mangle -A SOCKROCKET_IPV6_DEVICES -s "$device_source" -p "$device_proto" \
                    -j MARK --set-mark "$TUN_MARK" || { ipv6_proxy_stop; return 1; }
            done
        done
        ip6tables -t mangle -A "$IPV6_MANGLE" -i "$iface" -j SOCKROCKET_IPV6_DEVICES || { ipv6_proxy_stop; return 1; }
        ip6tables -t mangle -A "$IPV6_MANGLE" -m mark --mark "$TUN_MARK" -j RETURN || { ipv6_proxy_stop; return 1; }
    fi
    # Real-address QUIC must reach userspace too. A stream-only proxy responds
    # with port unreachable there, allowing TCP fallback without bypassing
    # global or destination rules.
    for proto in tcp udp; do
        ip6tables -t mangle -A "$IPV6_MANGLE" -i "$iface" -p "$proto" -j MARK --set-mark "$TUN_MARK" || { ipv6_proxy_stop; return 1; }
    done
    # Never mark ICMPv6: preserve NDP, router advertisements and PMTU.
    if ! {
        ip -6 route replace default dev "$tun" table "$SOCKROCKET_RT_TABLE" &&
        ip -6 rule add fwmark "$TUN_MARK" table "$SOCKROCKET_RT_TABLE" &&
        ip -6 route replace 2001:2:0:4765::/96 dev "$tun" &&
        ip6tables -A "$IPV6_FORWARD" -i "$tun" -o "$iface" -j ACCEPT
    }; then ipv6_proxy_stop; return 1; fi
    # QUIC is forwarded with other UDP. Userspace accepts, directs, or
    # rejects that one flow. A group rule must not reject the rest of the LAN.
    if ! {
        ip6tables -A "$IPV6_FORWARD" -i "$iface" -o "$tun" -p tcp -j ACCEPT &&
        ip6tables -A "$IPV6_FORWARD" -i "$iface" -o "$tun" -p udp -j ACCEPT &&
        ip6tables -I FORWARD 1 -j "$IPV6_FORWARD" &&
        ip6tables -t mangle -A PREROUTING -j "$IPV6_MANGLE"
    }; then ipv6_proxy_stop; return 1; fi
    ipv6_proxy_verify || { ipv6_proxy_stop; return 1; }
    # The dual-stack jump now precedes the guard; dropping the guard last
    # means no forwarded IPv6 packet ever saw neither.
    ipv6_compat_guard_stop || { ipv6_proxy_stop; return 1; }
    ipv6_clear_fallback
    ok "IPv6 dual-stack applied (LAN TCP+UDP to owned TUN policy table)"
}

ipv6_proxy_verify() {
    local iface tun
    iface=$(ipv6_lan_interface) || return 1
    tun=$(detect_tun_dev) || return 1
    ipv6_forward_jump_ok "$IPV6_FORWARD" || return 1
    ip6tables -t mangle -C PREROUTING -j "$IPV6_MANGLE" 2>/dev/null &&
    ip6tables -t mangle -C "$IPV6_MANGLE" -i "$iface" -p tcp -j MARK --set-mark "$TUN_MARK" 2>/dev/null &&
    ip6tables -t mangle -C "$IPV6_MANGLE" -i "$iface" -p udp -j MARK --set-mark "$TUN_MARK" 2>/dev/null &&
    ip6tables -C FORWARD -j "$IPV6_FORWARD" 2>/dev/null &&
    ip6tables -C "$IPV6_FORWARD" -i "$tun" -o "$iface" -j ACCEPT 2>/dev/null &&
    ip -6 rule show 2>/dev/null | grep -Eq "fwmark ${TUN_MARK}(/0xffffffff)? .*lookup ${SOCKROCKET_RT_TABLE}([[:space:]]|$)" || return 1
    ip -6 route show table "$SOCKROCKET_RT_TABLE" 2>/dev/null | grep -Eq "^default dev $tun( |$)" || return 1
    ip -6 route show 2001:2:0:4765::/96 2>/dev/null | grep -Eq "^2001:2:0:4765::/96 dev $tun( |$)"
}

# Client source prefixes for whichever LAN interface this host is using.
# Connected routes may be kernel, static, or router-advertisement routes,
# and some firmwares only show the prefix on the address itself.
ipv6_client_prefixes() {
    local iface="$1"
    {
        ip -6 route show dev "$iface" 2>/dev/null | awk '$1 ~ /:/ && $1 ~ /\// {print $1}'
        ip -6 addr show dev "$iface" scope global 2>/dev/null | awk '/inet6 / {print $2}'
    } | awk '
        $1 ~ /^fe80:/ || $1 ~ /^ff[0-9a-fA-F][0-9a-fA-F]:/ { next }
        $1 ~ /^fd9a:4c2e:17b0:/ { next }
        $1 ~ /\/128$/ { next }
        !seen[$1]++ { print }
    '
}

# Sockrocket's DNS listener, not dnsmasq. IPv6 REDIRECT delivers to this
# port; dnsmasq on many firmwares accepts only IPv4.
ipv6_dns_port() {
    local port
    port=$(get_conf dns_port)
    port=${port:-5300}
    case "$port" in
        ''|*[!0-9]*) return 1 ;;
    esac
    if [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
        return 1
    fi
    printf '%s\n' "$port"
}

ipv6_dns_guard_stop() {
    local port="" iface
    if [ -f "$(ipv6_state dns_port)" ]; then
        port=$(cat "$(ipv6_state dns_port)" 2>/dev/null || true)
    fi
    iface=$(ipv6_lan_interface 2>/dev/null) || iface=br0
    if [ -n "$port" ]; then
        ip6tables -D INPUT -p udp --dport "$port" ! -i "$iface" -j DROP 2>/dev/null || true
        ip6tables -D INPUT -p tcp --dport "$port" ! -i "$iface" -j DROP 2>/dev/null || true
    fi
    rm -f "$(ipv6_state dns_port)"
    return 0
}

# Close the IPv6 listener on every interface except the LAN. Redirected
# client queries arrive on that interface; WAN clients must not reach the port.
ipv6_dns_guard_start() {
    local iface="$1" port="$2"
    ipv6_dns_guard_stop
    printf '%s\n' "$port" > "$(ipv6_state dns_port)"
    if ! {
        ip6tables -I INPUT 1 -p udp --dport "$port" ! -i "$iface" -j DROP &&
        ip6tables -I INPUT 1 -p tcp --dport "$port" ! -i "$iface" -j DROP
    }; then
        ipv6_dns_guard_stop
        return 1
    fi
}

ipv6_dns_installed_prefixes() {
    local iface
    iface=$(ipv6_lan_interface) || return 1
    ipv6_client_prefixes "$iface" | awk '$1 !~ /^fe80:/' | sort
}

ipv6_dns_prefixes_match() {
    local now saved
    now=$(ipv6_dns_installed_prefixes) || return 1
    saved=$(sort "$(ipv6_state dns_prefixes)" 2>/dev/null || true)
    [ -n "$now" ] && [ "$now" = "$saved" ]
}

ipv6_dns_attached() {
    local port
    port=$(ipv6_dns_port) || return 1
    [ "$(cat "$(ipv6_state dns_port)" 2>/dev/null || true)" = "$port" ] || return 1
    ip6tables -t nat -C PREROUTING -j "$IPV6_DNS" 2>/dev/null || return 1
    ipv6_dns_prefixes_match
}

ipv6_dns_mark_active() {
    printf '1\n' > "$(ipv6_state dns_active)"
}

ipv6_dns_mark_inactive() {
    rm -f "$(ipv6_state dns_active)"
}

# The daemon opens its IPv6 DNS listener only as a transparent TUN gateway.
# Redirecting IPv6 :53 anywhere else would point clients at a closed port.
ipv6_transparent_ready() {
    case "$(get_conf transparent_proxy | tr 'A-Z' 'a-z')" in
        false|no|0) return 1 ;;
    esac
    [ -n "$(detect_tun_dev)" ]
}

# True when the jump sits before every ACCEPT in FORWARD. Other add-ons may
# insert rules above ours; only an earlier ACCEPT lets traffic skip us.
ipv6_forward_jump_ok() {
    ip6tables -S FORWARD 2>/dev/null | awk -v want="-A FORWARD -j $1" '
        $0 == want { found = 1; exit }
        $1 == "-A" && / -j ACCEPT( |$)/ { exit }
        END { exit !found }
    '
}

ipv6_dns_start() {
    # The daemon answers AAAA only while this file exists. Clear it before a
    # mode that does not intercept IPv6 DNS, so saved dual_stack cannot leak
    # IPv6 answers through an IPv4 firewall.
    if [ "$(get_conf ipv6_mode)" != dual_stack ] || ! ipv6_transparent_ready; then
        ipv6_dns_stop
        return 0
    fi
    local iface dest prefix prefixes port
    iface=$(ipv6_lan_interface) || return 1
    port=$(ipv6_dns_port) || {
        err "IPv6 DNS interception needs a numeric dns_port"
        return 1
    }
    # On-link prefixes only. An interface-wide redirect would also catch
    # forwarded traffic that merely arrives on the LAN interface.
    prefixes=$(ipv6_client_prefixes "$iface")
    [ -n "$prefixes" ] || {
        err "IPv6 DNS interception needs an on-link LAN prefix"
        return 1
    }
    if ! {
        { ip6tables -t nat -N "$IPV6_DNS" 2>/dev/null || ip6tables -t nat -F "$IPV6_DNS"; }
    }; then
        err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
        return 1
    fi
    # Queries already addressed to this router, including its link-local
    # address, stay on dnsmasq. The address string from `ip` includes the
    # on-link prefix length; matching that would exempt the whole LAN.
    # Same exception the IPv4 chain gives ROUTER_IP, so match the host only.
    for dest in $(ip -6 addr show dev "$iface" scope global 2>/dev/null | awk '/inet6 / {print $2}'); do
        local host="${dest%%/*}"
        case "$host" in
            *:*) ;;
            *) err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"; return 1 ;;
        esac
        ip6tables -t nat -A "$IPV6_DNS" -d "$host/128" -j RETURN || {
            err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
            return 1
        }
    done
    ip6tables -t nat -A "$IPV6_DNS" -d fe80::/10 -j RETURN || {
        err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
        return 1
    }
    local redirected=0
    for prefix in $prefixes; do
        # Link-local kernel routes are not LAN client sources. Redirecting
        # them would catch non-global traffic that arrived on the bridge.
        case "$prefix" in
            fe80:*) continue ;;
        esac
        ip6tables -t nat -A "$IPV6_DNS" -s "$prefix" -p udp --dport 53 -j REDIRECT --to-ports "$port" &&
        ip6tables -t nat -A "$IPV6_DNS" -s "$prefix" -p tcp --dport 53 -j REDIRECT --to-ports "$port" || {
            err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
            return 1
        }
        redirected=1
    done
    if [ "$redirected" != 1 ]; then
        err "IPv6 DNS interception needs a global on-link LAN prefix"
        return 1
    fi
    ipv6_dns_installed_prefixes > "$(ipv6_state dns_prefixes)" || {
        err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
        return 1
    }
    ipv6_dns_guard_start "$iface" "$port" || {
        err "IPv6 DNS listener could not be limited to the LAN interface"
        return 1
    }
    { ip6tables -t nat -C PREROUTING -j "$IPV6_DNS" 2>/dev/null ||
      ip6tables -t nat -A PREROUTING -j "$IPV6_DNS"; } || {
        err "IPv6 DNS interception unavailable; refusing incomplete DNS setup"
        return 1
    }
    ipv6_dns_sync_marker
}

# Fake IPv6 answers are useful only while LAN IPv6 reaches the TUN. In the
# compatibility fallback the guard rejects them and every client stalls.
ipv6_dns_sync_marker() {
    if ipv6_dns_attached && ipv6_proxy_verify; then
        ipv6_dns_mark_active
    else
        ipv6_dns_mark_inactive
    fi
}

ipv6_dns_stop() {
    local count=0
    ipv6_dns_mark_inactive
    rm -f "$(ipv6_state dns_prefixes)"
    ipv6_dns_guard_stop
    while [ "$count" -lt 32 ] && ip6tables -t nat -D PREROUTING -j "$IPV6_DNS" 2>/dev/null; do count=$((count+1)); done
    ip6tables -t nat -C PREROUTING -j "$IPV6_DNS" 2>/dev/null && return 1
    ip6tables -t nat -F "$IPV6_DNS" 2>/dev/null || true
    ip6tables -t nat -X "$IPV6_DNS" 2>/dev/null || true
    return 0
}

# Replace only the IPv6 policy. IPv4 mangle, NAT and DNS chains stay in place,
# including when dual-stack cannot be completed on this host.
do_ipv6_apply() {
    # Without the TUN gateway there is no IPv6 proxy path or IPv6 listener.
    case "$(get_conf transparent_proxy | tr 'A-Z' 'a-z')" in
        false|no|0)
            ipv6_clear_fallback
            ipv6_dns_stop || return 1
            ipv6_guard_stop
            return $?
            ;;
    esac
    if [ -z "$(detect_tun_dev)" ]; then
        # Mid-restart: leave the current guard; the next pass completes it.
        ipv6_dns_mark_inactive
        err "IPv6 policy not applied; TUN device is not ready"
        return 1
    fi
    if [ "$(get_conf ipv6_mode)" = "dual_stack" ]; then
        if ! ipv6_try_dual_stack; then
            # A partial ip6tables failure may have left neither policy.
            ipv6_dns_mark_inactive
            ipv6_compat_guard_start || err "IPv6 protection could not be restored; retrying on the next pass"
            return 1
        fi
        if [ "$(get_conf dns_hijack)" = "true" ]; then
            if ! ipv6_dns_attached; then
                ipv6_dns_stop || return 1
                if ! ipv6_dns_start; then
                    ipv6_dns_stop || return 1
                    ipv6_note_fallback "IPv6 DNS interception unavailable; IPv4 DNS hijack kept"
                fi
            fi
        else
            ipv6_dns_stop || return 1
        fi
        # The tmpfs marker can be lost while rules survive; restore it.
        ipv6_dns_sync_marker
        if ipv6_proxy_verify && { [ "$(get_conf dns_hijack)" != "true" ] || ipv6_dns_attached; }; then
            ipv6_clear_fallback
        fi
        return 0
    fi
    ipv6_clear_fallback
    ipv6_proxy_stop || return 1
    ipv6_dns_stop || return 1
    ipv6_compat_guard_start
}

# Compatibility mode does not proxy IPv6. When the firmware later enables
# IPv6, install the reject guard without waiting for a firewall rebuild.
# Turning firmware IPv6 back off removes that guard.
do_ipv6_guard() {
    case "$(get_conf transparent_proxy | tr 'A-Z' 'a-z')" in
        false|no|0)
            ipv6_dns_stop || return 1
            ipv6_guard_stop
            return $?
            ;;
    esac
    if [ "$(get_conf ipv6_mode)" = "dual_stack" ]; then
        return 0
    fi
    if ! ipv6_guard_needed; then
        ipv6_compat_guard_stop
        return $?
    fi
    ipv6_compat_guard_start
}

# Node addresses used by this router, and by a LAN device that runs its own
# proxy against the same servers. Those flows must leave directly: marking
# them into the TUN wraps the proxy connection a second time.
proxy_endpoint_returns() {
    local family="$1" dest endpoints
    [ -x "$SOCKROCKET_DIR/sockrocket-cli" ] || return 0
    endpoints=$("$SOCKROCKET_DIR/sockrocket-cli" --dump-proxy-endpoints "$SOCKROCKET_CONF" 2>/dev/null) || return 0
    for dest in $endpoints; do
        case "$family:$dest" in
            4:*.*.*.*)
                _valid_ip4 "$dest" || continue
                case "$dest" in
                    0.*|10.*|127.*|169.254.*|192.168.*|198.18.*|198.19.*|224.*|240.*) continue ;;
                    100.*)
                        # 100.64.0.0/10 is already returned with the private ranges.
                        case "$dest" in
                            100.6[4-9].*|100.[7-9][0-9].*|100.1[0-2][0-9].*) continue ;;
                        esac
                        ;;
                    172.*)
                        case "$dest" in
                            172.1[6-9].*|172.2[0-9].*|172.3[0-1].*) continue ;;
                        esac
                        ;;
                esac
                iptables -t mangle -A "$MANGLE_CHAIN" -d "$dest" -j RETURN || return 1
                ;;
            6:*:*)
                case "$dest" in
                    *[!0-9A-Fa-f:]*) continue ;;
                    ::|::1|fe80:*|2001:2:0:4765:*) continue ;;
                esac
                ip6tables -t mangle -A "$IPV6_MANGLE" -d "$dest" -j RETURN || return 1
                ;;
        esac
    done
    return 0
}

flush_chains() {
    ipv6_guard_stop || return 1
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
    iptables -D FORWARD -j SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -F SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -X SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -t mangle -F SOCKROCKET_DEVICES 2>/dev/null || true
    iptables -t mangle -X SOCKROCKET_DEVICES 2>/dev/null || true
    info "Flushed existing Sockrocket proxy iptables chains"
}

do_start() {
    if ! load_lan_scope; then
        err "Invalid primary LAN address, netmask or interface — transparent proxy NOT enabled"
        return 1
    fi
    # No-op cleanly when TUN is absent (firewall-start after reboot before the
    # daemon is up). Do not flush anything in that case — preserves DNS NAT.
    TUN_DEV=$(detect_tun_dev)
    if [ -z "$TUN_DEV" ]; then
        err "TUN device not found — transparent proxy NOT enabled (start sockrocket-cli with TUN first)"
        return 1
    fi
    flush_chains || return 1

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

    if ! iptables -t mangle -N "$MANGLE_CHAIN" 2>/dev/null \
        && ! iptables -t mangle -F "$MANGLE_CHAIN"; then
        err "Cannot create TUN mark chain"
        return 1
    fi
    local dest proto
    for dest in "$ROUTER_IP" 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 \
        100.64.0.0/10 127.0.0.0/8 169.254.0.0/16 224.0.0.0/4 240.0.0.0/4; do
        iptables -t mangle -A "$MANGLE_CHAIN" -d "$dest" -j RETURN || {
            flush_chains
            return 1
        }
    done
    # Before device marks, so a client that proxies through one of our nodes
    # is not captured again. Fake-IP and private ranges never qualify.
    proxy_endpoint_returns 4 || { flush_chains; return 1; }
    # Only selected LAN sources bypass kernel CN acceleration and reach the
    # device-aware router. Private/router destinations remain excluded above.
    local device_sources device_source device_proto
    device_sources=$("$SOCKROCKET_DIR/sockrocket-cli" --dump-rule-sources "$SOCKROCKET_CONF" 2>/dev/null) || {
        flush_chains
        err "Cannot validate device rule sources; transparent proxy not attached"
        return 1
    }
    if [ -n "$device_sources" ]; then
        if ! iptables -t mangle -N SOCKROCKET_DEVICES 2>/dev/null \
            && ! iptables -t mangle -F SOCKROCKET_DEVICES; then
            flush_chains
            return 1
        fi
        for device_source in $device_sources; do
            for device_proto in tcp udp; do
                iptables -t mangle -A SOCKROCKET_DEVICES -s "$device_source" -p "$device_proto" \
                    -j MARK --set-mark "$TUN_MARK" || { flush_chains; return 1; }
            done
        done
        if ! iptables -t mangle -A "$MANGLE_CHAIN" -i "$LAN_IFACE" -s "$LAN_SUBNET" -j SOCKROCKET_DEVICES \
            || ! iptables -t mangle -A "$MANGLE_CHAIN" -m mark --mark "$TUN_MARK" -j RETURN; then
            flush_chains
            return 1
        fi
    fi

    # Real-address QUIC must also reach the router. Bypassing all non-Fake-IP
    # addresses leaks global/reject traffic when a client uses DoH or a cached
    # real address and the selected proxy has no UDP transport.
    # Optional: CN-range bypass BEFORE the MARK rules (hardware NAT for
    # domestic traffic). Gated by the cn_ipset_direct config toggle.
    if [ "$(get_conf cn_ipset_direct)" = "true" ]; then
        do_ipset_start
    fi
    # Mark all other LAN-originated TCP+UDP; policy routing sends them into TUN.
    for proto in tcp udp; do
        iptables -t mangle -A "$MANGLE_CHAIN" -i "$LAN_IFACE" -s "$LAN_SUBNET" -p "$proto" \
            -j MARK --set-mark "$TUN_MARK" || {
                flush_chains
                return 1
            }
    done
    # Stage routing before exposing MARK rules to LAN packets. Repeated
    # firewall/WAN events must not accumulate identical policy rules.
    if ! ip route replace default dev "$TUN_DEV" table "$SOCKROCKET_RT_TABLE" 2>/dev/null; then
        flush_chains
        err "TUN route setup failed — LAN marking remains disabled"
        return 1
    fi
    if ! ip rule show 2>/dev/null | grep -Eq "fwmark ${TUN_MARK}(/0xffffffff)? .*lookup ${SOCKROCKET_RT_TABLE}([[:space:]]|$)"; then
        if ! ip rule add fwmark "$TUN_MARK" table "$SOCKROCKET_RT_TABLE" 2>/dev/null; then
            flush_chains
            err "TUN policy rule setup failed — LAN marking remains disabled"
            return 1
        fi
    fi


    # Return-path forwarding: replies sockrocket-cli injects into the TUN (src =
    # public internet IP, dst = LAN client) traverse the filter FORWARD
    # chain, whose Merlin default ends in DROP after a `DROP state INVALID`
    # rule. They normally pass via the ESTABLISHED accept — but conntrack
    # entries expire while the userspace ipstack keeps long-lived sessions
    # (its own timeout is longer), and every reply after expiry is INVALID
    # and dropped: the flow hangs mid-transfer with no RST either way.
    # Accept everything out of the TUN explicitly; the daemon only injects
    # packets for flows it accepted, so this grants no new reachability.
    if ! iptables -C FORWARD -i "$TUN_DEV" -j ACCEPT 2>/dev/null \
        && ! iptables -I FORWARD 1 -i "$TUN_DEV" -j ACCEPT; then
        flush_chains
        err "TUN return-path forwarding unavailable"
        return 1
    fi

    # Fake-IP replies injected through TUN need a reverse route back to TUN.
    # An unreachable route breaks reverse-path validation on Merlin even when
    # marked outbound packets use our private table. Keep unsupported UDP's
    # explicit REJECT below rather than rejecting the entire Fake-IP prefix.
    ip route del blackhole 198.18.0.0/16 2>/dev/null || true
    ip route del unreachable 198.18.0.0/16 2>/dev/null || true
    if ! ip route replace 198.18.0.0/16 dev "$TUN_DEV"; then
        flush_chains
        err "Fake-IP reverse route unavailable"
        return 1
    fi
    iptables -D FORWARD -s "$LAN_SUBNET" -d 198.18.0.0/16 -p udp \
        -j REJECT --reject-with icmp-port-unreachable 2>/dev/null || true
    iptables -D FORWARD -j SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -F SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -X SOCKROCKET_UDP_GUARD 2>/dev/null || true
    # Keep TUN return-path ACCEPT at the very top.
    iptables -D FORWARD -i "$TUN_DEV" -j ACCEPT 2>/dev/null || true
    if ! iptables -I FORWARD 1 -i "$TUN_DEV" -j ACCEPT; then
        flush_chains
        err "TUN return-path reordering failed"
        return 1
    fi

    if ! ipv6_guard_start; then
        flush_chains
        return 1
    fi

    if ! iptables -t mangle -C PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null \
        && ! iptables -t mangle -A PREROUTING -j "$MANGLE_CHAIN"; then
        flush_chains
        err "TUN mark-chain attach failed"
        return 1
    fi

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
    if ! load_lan_scope; then
        err "Invalid primary LAN address, netmask or interface — DNS interception NOT enabled"
        return 1
    fi
    if ! {
        { iptables -t nat -N "$DNS_CHAIN" 2>/dev/null || iptables -t nat -F "$DNS_CHAIN"; } &&
        iptables -t nat -A "$DNS_CHAIN" -d "$ROUTER_IP" -j RETURN &&
        iptables -t nat -A "$DNS_CHAIN" -i "$LAN_IFACE" -s "$LAN_SUBNET" -p udp --dport 53 -j REDIRECT --to-ports 53 &&
        iptables -t nat -A "$DNS_CHAIN" -i "$LAN_IFACE" -s "$LAN_SUBNET" -p tcp --dport 53 -j REDIRECT --to-ports 53 &&
        { iptables -t nat -C PREROUTING -j "$DNS_CHAIN" 2>/dev/null ||
          iptables -t nat -A PREROUTING -j "$DNS_CHAIN"; }
    }; then
        do_dns_stop
        return 1
    fi
    if ! ipv6_dns_start; then
        # IPv4 hijack is already installed. Removing it here would send the
        # LAN back to direct DNS because IPv6 could not be completed.
        ipv6_dns_stop || return 1
        ipv6_note_fallback "IPv6 DNS interception unavailable; IPv4 DNS hijack kept"
        ok "DNS hijack applied for IPv4; IPv6 DNS interception pending"
        return 0
    fi
    ok "DNS hijack applied (LAN :53 → router dnsmasq)"
}

do_dns_stop() {
    ipv6_dns_stop || return 1
    iptables -t nat -D PREROUTING -j "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -F "$DNS_CHAIN" 2>/dev/null || true
    iptables -t nat -X "$DNS_CHAIN" 2>/dev/null || true
}

do_stop() {
    info "Removing transparent proxy rules..."
    local cleanup_rc=0
    # Cleanup must still work after NVRAM has become invalid. Only the legacy
    # source-specific rule needs the old LAN scope; owned chains do not.
    load_lan_scope || LAN_SUBNET=""
    ipv6_guard_stop || cleanup_rc=1

    # TCP rules
    iptables -t nat -D PREROUTING -j "$CHAIN"     2>/dev/null || true
    iptables -t nat -F "$CHAIN" 2>/dev/null || true
    iptables -t nat -X "$CHAIN" 2>/dev/null || true

    # DNS rules
    do_dns_stop || cleanup_rc=1

    # UDP/mangle rules
    iptables -t mangle -D PREROUTING -j "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -F "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -t mangle -X "$MANGLE_CHAIN" 2>/dev/null || true
    iptables -D FORWARD -j SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -F SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -X SOCKROCKET_UDP_GUARD 2>/dev/null || true
    iptables -t mangle -F SOCKROCKET_DEVICES 2>/dev/null || true
    iptables -t mangle -X SOCKROCKET_DEVICES 2>/dev/null || true

    # TUN return-path forward accept (see do_start). Also clear the legacy
    # tun0 rule older builds may have installed.
    iptables -D FORWARD -i sockrocket-tun -j ACCEPT 2>/dev/null || true
    iptables -D FORWARD -i tun0 -j ACCEPT 2>/dev/null || true
    # Fake-IP QUIC fail-fast reject (see do_start).
    if [ -n "$LAN_SUBNET" ]; then
        iptables -D FORWARD -s "$LAN_SUBNET" -d 198.18.0.0/16 -p udp \
            -j REJECT --reject-with icmp-port-unreachable 2>/dev/null || true
    fi

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

    # Cached Fake-IP answers cannot be routed after TUN is disabled. Return
    # an explicit unreachable error rather than silently blackholing them.
    # Full network restore removes this temporary route after DNS unpinning.
    ip route del 198.18.0.0/16 dev "${TUN_DEV:-sockrocket-tun}" 2>/dev/null || true
    ip route del blackhole 198.18.0.0/16 2>/dev/null || true
    ip route replace unreachable 198.18.0.0/16 2>/dev/null \
        || ip route add unreachable 198.18.0.0/16 2>/dev/null || true

    if [ "$cleanup_rc" != 0 ]; then
        err "Proxy rules removed with pending IPv6 cleanup"
        return 1
    fi
    ok "iptables rules removed"
}

do_status() {
    echo "=== IPv6 fallback protection ==="
    ip6tables -L "$IPV6_CHAIN" -n --line-numbers 2>/dev/null || echo "(not active; IPv6 disabled or proxy stopped)"
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
    local errors=0 TUN_DEV
    TUN_DEV=$(detect_tun_dev)
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
    # Check the return route as well as the marked outbound route.
    ip route show 2>/dev/null | grep -Eq "^198.18.0.0/16 dev $TUN_DEV( |$)" || {
        err "Fake-IP reverse route to TUN missing"
        errors=$((errors + 1))
    }
    if [ "$(get_conf cn_ipset_direct)" = "true" ]; then
        iptables -t mangle -L SOCKROCKET_MANGLE -n 2>/dev/null | grep -q "match-set $IPSET_NAME" || {
            err "cn_ipset_direct enabled but $IPSET_NAME bypass rule missing"
            errors=$((errors + 1))
        }
    fi
    ipv6_guard_verify || {
        err "IPv6 bypass protection missing or superseded"
        errors=$((errors + 1))
    }
    [ "$errors" -eq 0 ] && ok "iptables rules verified OK" && return 0
    err "iptables verification FAILED ($errors issue(s))"
    return 1
}

case "$1" in
    start)   do_start ;;
    stop)    do_stop ;;
    restart) do_stop; do_start ;;
    ipv6-apply) do_ipv6_apply ;;
    ipv6-guard) do_ipv6_guard ;;
    dns-start) do_dns_start ;;
    dns-stop)  do_dns_stop ;;
    ipset-on)  do_ipset_start ;;
    ipset-off) do_ipset_stop ;;
    status)  do_status ;;
    verify)  do_verify ;;
    *)
        echo "Usage: $0 {start|stop|restart|ipv6-apply|ipv6-guard|dns-start|dns-stop|ipset-on|ipset-off|status|verify}"
        exit 1
        ;;
esac
