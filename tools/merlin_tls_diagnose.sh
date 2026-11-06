#!/bin/sh
# Run on Merlin. Compare system DNS, LAN DNS + TUN, and SOCKS domain dialing.
# Does not change DNS, routing, nodes or the running service. TLS verification
# stays enabled. Each request is attempted once; failures are evidence.
set -eu
OUT=${1:?usage: merlin_tls_diagnose.sh output-directory [rounds]}
ROUNDS=${2:-5}
case "$ROUNDS" in ''|*[!0-9]*) exit 2;; esac
[ "$ROUNDS" -gt 0 ] && [ "$ROUNDS" -le 30 ] || exit 2
D=${SOCKROCKET_DIR:-/jffs/addons/sockrocket}
SOCKS_PORT=${SOCKROCKET_SOCKS_PORT:-1080}
TUN=${SOCKROCKET_TUN_INTERFACE:-sockrocket-tun}
umask 077
mkdir -p "$OUT"
mkdir "$OUT/collector.lock"
trap 'rmdir "$OUT/collector.lock"' EXIT
[ ! -e "$OUT/samples.csv" ] || exit 1
fingerprint() { sha256sum "$D/config.yaml" | awk '{print $1}'; }
fingerprint > "$OUT/config.before.sha256"
curl -V > "$OUT/curl-version.txt"

# BusyBox nslookup has no portable timeout option. Bound each query without
# replacing /etc/resolv.conf (which the router also uses to bootstrap nodes).
for host in www.google.com www.gstatic.com; do
    nslookup "$host" 127.0.0.1 > "$OUT/$host.dns.txt" 2>&1 &
    query_pid=$!
    (sleep 8; kill "$query_pid" 2>/dev/null || true) &
    timer_pid=$!
    rc=0
    wait "$query_pid" || rc=$?
    kill "$timer_pid" 2>/dev/null || true
    wait "$timer_pid" 2>/dev/null || true
    [ "$rc" -eq 0 ] || { echo "Local DNS lookup failed: $host" >&2; exit 1; }
    awk '
        /^Name:/ {answer=1; next}
        answer && /^Address/ {
            for (i=2; i<=NF; i++)
                if ($i ~ /^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$/) {print $i; exit}
        }
    ' "$OUT/$host.dns.txt" > "$OUT/$host.ip"
    [ -s "$OUT/$host.ip" ] || { echo "No local DNS IPv4 answer: $host" >&2; exit 1; }
done

printf '%s\n' 'run,host,path,start,pid,config_before,config_after,exit,http,dns,connect,tls,first,total,ip' > "$OUT/samples.csv"
i=1
while [ "$i" -le "$ROUNDS" ]; do
    for host in www.google.com www.gstatic.com; do
        ip=$(cat "$OUT/$host.ip")
        for path in system-tun localdns-tun socks-domain; do
            case "$path" in
                system-tun) set -- --interface "$TUN" --noproxy '*' ;;
                localdns-tun) set -- --interface "$TUN" --noproxy '*' --resolve "$host:443:$ip" ;;
                socks-domain) set -- --socks5-hostname "127.0.0.1:$SOCKS_PORT" --noproxy '' ;;
            esac
            before=$(fingerprint)
            pid=$(cat "$D/sockrocket.pid")
            start=$(date +%s)
            rc=0
            metrics=$(curl -4 -v -sS -o /dev/null --connect-timeout 5 --max-time 12 \
                "$@" -w '%{http_code},%{time_namelookup},%{time_connect},%{time_appconnect},%{time_starttransfer},%{time_total},%{remote_ip}' \
                "https://$host/generate_204" 2> "$OUT/$i.$host.$path.log") || rc=$?
            printf '%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
                "$i" "$host" "$path" "$start" "$pid" "$before" "$(fingerprint)" "$rc" "$metrics" >> "$OUT/samples.csv"
        done
    done
    i=$((i + 1))
done
fingerprint > "$OUT/config.after.sha256"
awk -F, 'NR>1 {n[$3]++; if ($8==0 && $9==204) ok[$3]++; if ($6!=$7) changed++}
    END {for (p in n) printf "%s: %d/%d passed\n",p,ok[p],n[p]; printf "Requests spanning config changes: %d\n",changed}' "$OUT/samples.csv"
if ! cmp -s "$OUT/config.before.sha256" "$OUT/config.after.sha256"; then
    echo 'Configuration changed during sampling; do not attribute all samples to one node.'
fi
