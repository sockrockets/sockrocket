#!/bin/sh
# Only idle, low-threshold IPv4 node entries with one source may expire.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
sed '/^case "\$1" in/,$d' "$root/merlin/scripts/iptables.sh" > "$fixture/functions.sh"
nvram() { :; }
. "$fixture/functions.sh"
SOCKROCKET_RUNTIME_DIR="$fixture"
printf '%s\n' 192.0.2.1 192.0.2.2 192.0.2.3 192.0.2.4 192.0.2.5 > "$fixture/node-endpoints"
ip() {
    case "$*" in
        'tcp_metrics show')
            printf '%s\n' \
                '192.0.2.1 age 60.0sec ssthresh 7' \
                '192.0.2.2 age 59.9sec ssthresh 7' \
                '192.0.2.3 age 120sec ssthresh 33' \
                '192.0.2.4 age 120sec cwnd 15' \
                '192.0.2.5 age 120sec ssthresh 7 source 192.0.2.100' \
                '192.0.2.5 age 1sec ssthresh 63 source 192.0.2.101' \
                '192.0.2.10 age 120sec ssthresh 7';;
        *) echo "$*" >> "$fixture/actions";;
    esac
}
repair_node_tcp_metrics
test "$(cat "$fixture/actions")" = 'tcp_metrics delete 192.0.2.1'
rm "$fixture/actions" "$fixture/node-endpoints"
repair_node_tcp_metrics
test ! -e "$fixture/actions"
echo 'PASS: scoped TCP metrics expiry'
