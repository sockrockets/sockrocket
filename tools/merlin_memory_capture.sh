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
printf '%s\n' 'unix,uptime,pid,rss_kib,hwm_kib,anon_kib,available_kib,slab_kib,unreclaimable_kib,tcp_payload_bytes,packet_bytes,active,health_sampled_at,total_kib,available_source,rss_pct' > "$OUT/samples.csv"
json_number() { sed -n "s/.*\"$1\":\([0-9][0-9]*\).*/\1/p" /tmp/sockrocket_health.json 2>/dev/null || true; }
snapshot() {
    [ ! -e "$1" ] || return 0
    {
        date -u
        for file in /proc/meminfo /proc/net/sockstat /proc/slabinfo; do
            cat "$file" 2>/dev/null || true
        done
        # Keep size counters only. status contains IDs and smaps_rollup starts
        # with a process address range.
        for file in "/proc/$pid/status" "/proc/$pid/smaps_rollup"; do
            awk 'NF == 3 && $1 ~ /^[A-Za-z_0-9]+:$/ && $2 ~ /^[0-9]+$/ && $3 == "kB"' "$file" 2>/dev/null || true
        done
    } > "$1"
}

low_samples=0
i=0
while [ "$i" -lt "$SAMPLES" ] && [ ! -e "$OUT/stop" ]; do
    pid=$(cat /jffs/addons/sockrocket/sockrocket.pid 2>/dev/null || true)
    case "$pid" in ''|*[!0-9]*) pid=0;; esac
    # A process may exit between samples. Keep the system-wide sample if it does.
    process=$(awk '
        $2 ~ /^[0-9]+$/ { kb[$1]=$2 }
        END { printf "%s,%s,%s", kb["VmRSS:"], kb["VmHWM:"], kb["RssAnon:"] }
    ' "/proc/$pid/status" 2>/dev/null) || process=',,'
    IFS=, read -r rss hwm anon <<EOF
$process
EOF
    memory=$(awk -v rss="$rss" '
        $2 ~ /^[0-9]+$/ { kb[$1]=$2 }
        END {
            total=kb["MemTotal:"]+0
            source="unknown"
            if (total > 0) {
                if ("MemAvailable:" in kb) { avail=kb["MemAvailable:"]; source="measured" }
                else if ("MemFree:" in kb && "Buffers:" in kb && "Cached:" in kb) {
                    avail=kb["MemFree:"]+kb["Buffers:"]+kb["Cached:"]; source="estimated"
                }
                if (avail > total) avail=total
            }
            printf "%s,%s,%s,%s,%s,%s,%d", (source=="unknown" ? "" : avail), kb["Slab:"], kb["SUnreclaim:"], (total ? total : ""), source, (total && rss != "" ? sprintf("%.1f",100*rss/total) : ""), (source!="unknown" && avail*100 <= total*15)
        }' /proc/meminfo) || memory=',,,,unknown,,0'
    IFS=, read -r available slab unreclaimable total source rss_pct low <<EOF
$memory
EOF
    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
        "$(date +%s)" "$(cut -d ' ' -f 1 /proc/uptime)" "$pid" "$rss" "$hwm" "$anon" \
        "$available" "$slab" "$unreclaimable" "$(json_number tcp_payload_bytes)" \
        "$(json_number packet_bytes)" "$(json_number active)" "$(json_number sampled_at)" \
        "$total" "$source" "$rss_pct" >> "$OUT/samples.csv"
    for threshold in 131072 262144 524288; do
        if [ "${rss:-0}" -ge "$threshold" ]; then snapshot "$OUT/threshold-$threshold.txt"; fi
    done
    if [ "$low" = 1 ]; then
        low_samples=$((low_samples+1))
        if [ "$low_samples" -eq 3 ]; then snapshot "$OUT/low-available.txt"; fi
    else
        low_samples=0
    fi
    i=$((i + 1))
    [ "$i" -ge "$SAMPLES" ] || sleep "$INTERVAL"
done
date -u > "$OUT/completed"
