#!/bin/sh
# Stateful command mocks: never call a host firewall or mutate host routes.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
FIXTURE=$(mktemp -d)
trap 'rm -rf "$FIXTURE"' EXIT
logger() { :; }
nvram() { case "$2" in lan_ifname) echo br0;; ipv6_service) echo native;; *) echo 192.0.2.1;; esac; }
sed '/^case "\$1" in/,$d' "$ROOT/merlin/scripts/iptables.sh" > "$FIXTURE/functions.sh"
. "$FIXTURE/functions.sh"
SOCKROCKET_DIR="$FIXTURE/plugin"
SOCKROCKET_RUNTIME_DIR="$FIXTURE/run"
SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
mkdir -p "$SOCKROCKET_DIR"
printf 'ipv6_mode: dual_stack\ndns_hijack: true\n' > "$SOCKROCKET_CONF"
printf '#!/bin/sh\nexit 0\n' > "$SOCKROCKET_DIR/sockrocket-cli"
chmod +x "$SOCKROCKET_DIR/sockrocket-cli"
CALLS="$FIXTURE/calls"
FAIL_ROUTE=0
FAIL_DNS=0
FORWARDING=1
detect_tun_dev() { echo sockrocket-tun; }
cat() { case "$1" in /proc/sys/net/ipv6/conf/all/forwarding) echo "$FORWARDING";; *) /bin/cat "$@";; esac; }
iptables() { printf 'iptables %s\n' "$*" >> "$CALLS"; case " $* " in *' -C '*) return 1;; esac; return 0; }
# FORWARD is an ordered list of owned jumps, plus an optional foreign ACCEPT
# placed at the top by another add-on.
FORWARD_LIST="$FIXTURE/forward_list"
: > "$FORWARD_LIST"
WATCH_GAP=0
forward_changed() {
    # Once LAN IPv6 is protected, every FORWARD state must keep one of the
    # two owned policies, or forwarded IPv6 escapes natively.
    [ "$WATCH_GAP" = 1 ] || return 0
    if ! grep -qx 'SOCKROCKET_IPV6_FORWARD' "$FORWARD_LIST" &&
       { ! grep -qx 'SOCKROCKET_IPV6_GUARD' "$FORWARD_LIST" || [ ! -f "$FIXTURE/guard_rule" ]; }; then
        touch "$FIXTURE/gap"
    fi
}
ip6tables() {
    printf 'ip6tables %s\n' "$*" >> "$CALLS"
    local args="$*" target
    target=${args##* }
    case "$*" in
        '-S FORWARD')
            test ! -f "$FIXTURE/foreign_top" || echo '-A FORWARD -i wg0 -j SKYNET'
            while read -r chain; do echo "-A FORWARD -j $chain"; done < "$FORWARD_LIST";;
        '-I FORWARD 1 -j '*)
            { echo "$target"; /bin/cat "$FORWARD_LIST"; } > "$FORWARD_LIST.new"; mv "$FORWARD_LIST.new" "$FORWARD_LIST"
            test "$target" != SOCKROCKET_IPV6_FORWARD || touch "$FIXTURE/forward";;
        '-C FORWARD -j '*) grep -qx "$target" "$FORWARD_LIST";;
        '-D FORWARD -j '*)
            grep -qx "$target" "$FORWARD_LIST" || return 1
            grep -vx "$target" "$FORWARD_LIST" > "$FORWARD_LIST.new" || true; mv "$FORWARD_LIST.new" "$FORWARD_LIST"
            test "$target" != SOCKROCKET_IPV6_FORWARD || rm -f "$FIXTURE/forward"
            forward_changed;;
        '-A SOCKROCKET_IPV6_GUARD '*) touch "$FIXTURE/guard_rule";;
        '-C SOCKROCKET_IPV6_GUARD '*) test -f "$FIXTURE/guard_rule";;
        '-F SOCKROCKET_IPV6_GUARD'|'-X SOCKROCKET_IPV6_GUARD') rm -f "$FIXTURE/guard_rule"; forward_changed;;
        '-t mangle -A PREROUTING -j SOCKROCKET_IPV6_MANGLE') touch "$FIXTURE/mangle";;
        '-t mangle -C PREROUTING -j SOCKROCKET_IPV6_MANGLE') test -f "$FIXTURE/mangle";;
        '-t mangle -D PREROUTING -j SOCKROCKET_IPV6_MANGLE') test -f "$FIXTURE/mangle" || return 1; rm "$FIXTURE/mangle";;
        '-t nat -A PREROUTING -j SOCKROCKET_IPV6_DNS') touch "$FIXTURE/dns";;
        '-t nat -C PREROUTING -j SOCKROCKET_IPV6_DNS') test -f "$FIXTURE/dns";;
        '-t nat -D PREROUTING -j SOCKROCKET_IPV6_DNS') test -f "$FIXTURE/dns" || return 1; rm "$FIXTURE/dns";;
        '-t nat -A SOCKROCKET_IPV6_DNS '*) test "$FAIL_DNS" = 0;;
        *) return 0;;
    esac
}
guard_installed() { grep -qx SOCKROCKET_IPV6_GUARD "$FORWARD_LIST" && test -f "$FIXTURE/guard_rule"; }
ip() {
    printf 'ip %s\n' "$*" >> "$CALLS"
    case "$*" in
        '-6 addr show dev sockrocket-tun') test ! -f "$FIXTURE/tun6" || echo 'inet6 fd9a:4c2e:17b0::2/64 scope global';;
        '-6 addr replace fd9a:4c2e:17b0::2/64 dev sockrocket-tun') touch "$FIXTURE/tun6";;
        '-6 addr show dev br0') echo 'inet6 2001:db8:1::1/64';;
        '-6 addr show dev br0 scope global')
            if [ "${PREFIX_SET:-1}" = 2 ]; then echo 'inet6 2001:db8:9::1/64 scope global'
            else printf '%s\n' 'inet6 2001:db8:1::1/64 scope global' 'inet6 2001:db8:2::1/60 scope global'; fi;;
        '-6 route show dev br0')
            if [ "${PREFIX_SET:-1}" = 2 ]; then echo '2001:db8:9::/64 proto static'
            else printf '%s\n' 'fe80::/64 proto kernel' '2001:db8:1::/64 proto static'; fi;;
        '-6 route show dev br0 proto kernel') printf '%s\n' 'fe80::/64 proto kernel' '2001:db8:1::/64 proto kernel';;
        '-6 route replace default '*) test "$FAIL_ROUTE" = 0 || return 1; touch "$FIXTURE/route";;
        '-6 route del default '*) rm -f "$FIXTURE/route";;
        '-6 rule add '*) touch "$FIXTURE/rule";;
        '-6 rule del '*) test -f "$FIXTURE/rule" || return 1; rm "$FIXTURE/rule";;
        '-6 rule show') test ! -f "$FIXTURE/rule" || echo '32765: from all fwmark 0x4765 lookup 5370';;
        '-6 route show table 5370') test ! -f "$FIXTURE/route" || echo 'default dev sockrocket-tun';;
        '-6 route show 2001:2:0:4765::/96') echo '2001:2:0:4765::/96 dev sockrocket-tun';;
        *) return 0;;
    esac
}
fail() { echo "FAIL: $*" >&2; exit 1; }
ipv6_guard_stop || fail 'idempotent cleanup with no installed IPv6 rules'
ipv6_guard_start || fail 'dual-stack start'
grep -q -- '-6 addr replace fd9a:4c2e:17b0::2/64 dev sockrocket-tun' "$CALLS" || fail 'TUN IPv6 address not assigned'
ipv6_guard_verify || fail 'complete forwarding verification'
test -f "$FIXTURE/mangle" && test -f "$FIXTURE/route" && test -f "$FIXTURE/rule" || fail 'incomplete installation'
route_line=$(awk '/^ip -6 route replace default/{print NR;exit}' "$CALLS")
mark_line=$(awk '/^ip6tables -t mangle -A PREROUTING/{print NR;exit}' "$CALLS")
test "$route_line" -lt "$mark_line" || fail 'traffic exposed before routes'
grep -q -- '-d 2001:db8:1::1/64 -j RETURN' "$CALLS" || fail 'LAN prefix not exempt'
if grep -q -- '-p ipv6-icmp.*MARK' "$CALLS"; then fail 'ICMPv6 control traffic captured'; fi
do_dns_start || fail 'dual-stack DNS install'
test -f "$FIXTURE/dns" || fail 'IPv6 DNS missing'
test -f "$(ipv6_state dns_active)" || fail 'IPv6 answers not armed with interception'
grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -d 2001:db8:1::1/128 -j RETURN' "$CALLS" || fail 'router IPv6 DNS not exempt'
grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -d fe80::/10 -j RETURN' "$CALLS" || fail 'link-local DNS not exempt'
grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -s 2001:db8:1::/64 -p udp --dport 53 -j REDIRECT --to-ports 5300' "$CALLS" || fail 'IPv6 DNS source not limited'
grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -s 2001:db8:2::1/60 -p udp --dport 53 -j REDIRECT --to-ports 5300' "$CALLS" || fail 'address-only LAN prefix not intercepted'
grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -d 2001:db8:2::1/128 -j RETURN' "$CALLS" || fail 'second router address not exempt'
grep -q -- '-I INPUT 1 -p udp --dport 5300 ! -i br0 -j DROP' "$CALLS" || fail 'IPv6 DNS listener open off the LAN'
if grep -q -- '-s fe80::/64 -p udp --dport 53' "$CALLS"; then fail 'link-local prefix hijacked'; fi
if grep -q -- '-t nat -A SOCKROCKET_IPV6_DNS -i br0 -p udp --dport 53 -j REDIRECT' "$CALLS"; then fail 'IPv6 DNS hijacks the whole interface'; fi
v4_before=$(grep -c '^iptables ' "$CALLS" || true)
do_ipv6_apply || fail 'in-place IPv6 apply'
v4_after=$(grep -c '^iptables ' "$CALLS" || true)
test "$v4_before" = "$v4_after" || fail 'in-place apply touched IPv4 rules'
test ! -f "$(ipv6_state fallback)" || fail 'successful dual-stack left a fallback marker'
same=$(grep -c 'REDIRECT --to-ports 5300' "$CALLS" || true)
do_ipv6_apply || fail 'unchanged prefix reapply'
test "$same" = "$(grep -c 'REDIRECT --to-ports 5300' "$CALLS" || true)" || fail 'unchanged prefixes rebuilt DNS redirects'
PREFIX_SET=2
do_ipv6_apply || fail 'changed LAN prefix was not reapplied'
grep -q -- '-s 2001:db8:9::/64 -p udp --dport 53 -j REDIRECT --to-ports 5300' "$CALLS" || fail 'new LAN prefix not intercepted'
PREFIX_SET=1
ipv6_guard_stop || fail 'stop cleanup'
do_dns_stop || fail 'DNS cleanup'
test ! -f "$(ipv6_state dns_port)" || fail 'DNS listener guard port left behind'
test ! -f "$(ipv6_state dns_active)" || fail 'IPv6 answers left armed after DNS stop'
for owned in mangle forward route rule dns; do test ! -f "$FIXTURE/$owned" || fail "leftover $owned"; done
FAIL_ROUTE=1
ipv6_guard_start || fail 'route failure removed the IPv4 path'
for owned in mangle forward route rule; do test ! -f "$FIXTURE/$owned" || fail "partial failure leaked $owned"; done
test -f "$(ipv6_state fallback)" || fail 'route failure did not record IPv4 preservation'
guard_installed || fail 'route failure left LAN IPv6 without the reject guard'
# Watchdog retries while stuck in fallback must never open a gap.
WATCH_GAP=1
do_ipv6_apply || fail 'fallback retry'
guard_installed || fail 'fallback retry dropped the reject guard'
FAIL_ROUTE=0
do_ipv6_apply || fail 'recovery from fallback'
test -f "$FIXTURE/forward" || fail 'recovery did not install dual-stack'
! guard_installed || fail 'guard left after dual-stack recovered'
test ! -f "$FIXTURE/gap" || fail 'LAN IPv6 had neither guard nor dual-stack during rebuild'
# Another add-on at the top of FORWARD is not a reason to rebuild.
touch "$FIXTURE/foreign_top"
: > "$CALLS"
ipv6_proxy_verify || fail 'foreign top rule failed dual-stack verify'
do_ipv6_apply || fail 'apply with foreign top rule'
if grep -q -- '-D FORWARD -j SOCKROCKET_IPV6_FORWARD' "$CALLS"; then fail 'foreign top rule caused a teardown'; fi
rm -f "$FIXTURE/foreign_top"
WATCH_GAP=0
ipv6_guard_stop || fail 'cleanup after recovery'
rm -f "$(ipv6_state fallback)"
: > "$CALLS"
FAIL_DNS=1
do_dns_start || fail 'IPv6 NAT failure removed IPv4 DNS hijack'
test ! -f "$FIXTURE/dns" || fail 'failed DNS left hook'
grep -q '^iptables -t nat -A SOCKROCKET_DNS ' "$CALLS" || fail 'IPv4 DNS hijack missing after IPv6 DNS failure'
if grep -q '^iptables -t nat -D PREROUTING -j SOCKROCKET_DNS' "$CALLS"; then fail 'IPv6 DNS failure detached IPv4 hijack'; fi
test -f "$(ipv6_state fallback)" || fail 'IPv6 DNS failure did not record IPv4 preservation'
test ! -f "$(ipv6_state dns_active)" || fail 'failed IPv6 DNS armed AAAA answers'
FAIL_DNS=0
rm -f "$(ipv6_state fallback)"
FORWARDING=0
ipv6_guard_start || fail 'missing forwarding removed the IPv4 path'
test ! -f "$FIXTURE/mangle" || fail 'dual-stack marks installed without forwarding'
test -f "$(ipv6_state fallback)" || fail 'missing forwarding did not keep IPv4'
guard_installed || fail 'missing forwarding left LAN IPv6 unguarded'
# DNS interception succeeds but routing fell back: no fake AAAA answers.
do_dns_start || fail 'DNS start in routing fallback'
test ! -f "$(ipv6_state dns_active)" || fail 'AAAA armed while IPv6 forwarding is rejected'
FORWARDING=1
# Without transparent proxy the daemon has no IPv6 listener.
printf 'ipv6_mode: dual_stack\ndns_hijack: true\ntransparent_proxy: false\n' > "$SOCKROCKET_CONF"
do_ipv6_apply || fail 'apply without transparent proxy'
test ! -f "$FIXTURE/dns" || fail 'IPv6 DNS redirect kept without a listener'
! guard_installed || fail 'guard kept without transparent proxy'
do_dns_start || fail 'IPv4 DNS without transparent proxy'
test ! -f "$FIXTURE/dns" || fail 'IPv6 DNS redirected without transparent proxy'
# Device scopes are marked before stream-only QUIC stays on the kernel path.
printf 'ipv6_mode: dual_stack\ndns_hijack: true\ntransparent_proxy: true\n' > "$SOCKROCKET_CONF"
/bin/cat > "$SOCKROCKET_DIR/sockrocket-cli" <<'EOF'
#!/bin/sh
case "$1" in
    --dump-rule-sources-v6) printf '2001:db8:9::/64\n'; exit 0 ;;
    --supports-udp) exit 1 ;;
    *) exit 0 ;;
esac
EOF
chmod +x "$SOCKROCKET_DIR/sockrocket-cli"
: > "$CALLS"
FORWARDING=1
ipv6_guard_start || fail 'IPv6 device rules did not install'
device_line=$(awk '/SOCKROCKET_IPV6_DEVICES/{print NR; exit}' "$CALLS")
quic_line=$(awk '/--dport 443 -j RETURN/{print NR; exit}' "$CALLS")
test -n "$device_line" && test -n "$quic_line" && test "$device_line" -lt "$quic_line" || fail 'IPv6 device policy follows the QUIC bypass'
grep -q -- '-A SOCKROCKET_IPV6_DEVICES -s 2001:db8:9::/64 -p udp -j MARK' "$CALLS" || fail 'IPv6 device UDP was not marked'
ipv6_proxy_stop || fail 'IPv6 device chain cleanup'
grep -q -- '-X SOCKROCKET_IPV6_DEVICES' "$CALLS" || fail 'IPv6 device chain was left behind'
# A later firmware IPv6 enable must gain the compatibility guard without a restart.
printf 'ipv6_mode: compatibility\ntransparent_proxy: true\n' > "$SOCKROCKET_CONF"
ipv6_guard_stop || fail 'clear guard before compatibility check'
do_ipv6_guard || fail 'compatibility guard when firmware IPv6 is enabled'
guard_installed || fail 'enabling firmware IPv6 left LAN IPv6 unguarded'
nvram() { case "$2" in lan_ifname) echo br0 ;; ipv6_service) echo disabled ;; *) echo 192.0.2.1 ;; esac; }
do_ipv6_guard || fail 'compatibility guard while firmware IPv6 is disabled'
! guard_installed || fail 'guard kept after firmware IPv6 was disabled'
nvram() { case "$2" in lan_ifname) echo br0 ;; ipv6_service) echo native ;; *) echo 192.0.2.1 ;; esac; }
do_ipv6_guard || fail 'guard returns when firmware IPv6 returns'
guard_installed || fail 'firmware IPv6 returning did not restore the guard'
if grep -Eq '^ip -6 route (flush|replace default.*(via|table (main|100)))' "$CALLS"; then fail 'firmware route touched'; fi
echo 'PASS: IPv6 staged routing, LAN/ICMPv6 preservation, DNS interception, failure rollback and owned cleanup'
