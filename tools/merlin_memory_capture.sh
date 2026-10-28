#!/bin/sh
# Finite, read-only resource sampling for a Merlin soak test. No payload,
# destinations, credentials or node configuration is collected. Under 2 MiB
# at the default 26 hours; does not restart or reconfigure the service.
set -eu
OUT=${1:?usage: merlin_memory_capture.sh output-directory [samples] [interval-seconds]}
SAMPLES=${2:-1561}
INTERVAL=${3:-60}
case "$SAMPLES:$INTERVAL" in *[!0-9:]*|:*|*:) exit 2;; esac
[ "$SAMPLES" -gt 0 ] && [ "$SAMPLES" -le 1561 ] && [ "$INTERVAL" -gt 0 ] || exit 2
umask 077
mkdir -p "$OUT"
# Refuse an accidental second collector or truncation of existing evidence.
mkdir "$OUT/collector.lock" || exit 1
trap 'rmdir "$OUT/collector.lock"' EXIT
[ ! -e "$OUT/samples.csv" ] || exit 1
printf '%s\n' 'unix,uptime,pid,rss_kib,hwm_kib,anon_kib,available_kib,slab_kib,unreclaimable_kib,tcp_payload_bytes,packet_bytes,active,health_sampled_at' > "$OUT/samples.csv"
field() { awk -v key="$2" '$1 == key {print $2; found=1; exit} END {if (!found) print 0}' "$1"; }
json_number() { sed -n "s/.*\"$1\":\([0-9][0-9]*\).*/\1/p" /tmp/sockrocket_health.json 2>/dev/null || true; }
i=0
while [ "$i" -lt "$SAMPLES" ] && [ ! -e "$OUT/stop" ]; do
    pid=$(cat /jffs/addons/sockrocket/sockrocket.pid 2>/dev/null || true)
    case "$pid" in ''|*[!0-9]*) pid=0;; esac
    rss=0 hwm=0 anon=0
    if [ -r "/proc/$pid/status" ]; then
        rss=$(field "/proc/$pid/status" VmRSS:)
        hwm=$(field "/proc/$pid/status" VmHWM:)
        anon=$(field "/proc/$pid/status" RssAnon:)
    fi
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
        "$(date +%s)" "$(cut -d ' ' -f 1 /proc/uptime)" "$pid" "$rss" "$hwm" "$anon" \
        "$(field /proc/meminfo MemAvailable:)" "$(field /proc/meminfo Slab:)" \
        "$(field /proc/meminfo SUnreclaim:)" "$(json_number tcp_payload_bytes)" \
        "$(json_number packet_bytes)" "$(json_number active)" "$(json_number sampled_at)" >> "$OUT/samples.csv"
    for threshold in 131072 262144 524288; do
        if [ "$rss" -ge "$threshold" ] && [ ! -e "$OUT/threshold-$threshold.txt" ]; then
            {
                date -u
                cat /proc/meminfo /proc/net/sockstat
                cat "/proc/$pid/status" "/proc/$pid/smaps_rollup" 2>/dev/null || true
                cat /proc/slabinfo 2>/dev/null || true
            } > "$OUT/threshold-$threshold.txt"
        fi
    done
    i=$((i + 1))
    [ "$i" -ge "$SAMPLES" ] || sleep "$INTERVAL"
done
date -u > "$OUT/completed"
