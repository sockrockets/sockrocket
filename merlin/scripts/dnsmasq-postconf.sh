#!/bin/sh
# Point only the main LAN dnsmasq at Sockrocket.
# Merlin calls this from dnsmasq.postconf with the generated config as $1.
# SDN instances use dnsmasq-sdn.postconf; a second argument is their index
# and must be left on the firmware resolv, including a guest network with
# its own DNS. fancyss does the same edit on the main config: drop
# servers-file and resolv-file, then use server=127.0.0.1#port. That does
# not need a stock servers-file= line, so a full dnsmasq.conf replacement
# or a spaced key still isolates the main LAN.
config="$1"
sdn_index="$2"
# nvram exists on the router and not in the test sandbox. One syslog line
# records the path firmware passed, which the control script may not be reading.
postconf_log() {
    command -v nvram >/dev/null 2>&1 || return 0
    logger -t sockrocket -p daemon.info "$*" 2>/dev/null || true
}
[ -n "$sdn_index" ] && { postconf_log "Merlin 3006 DNS postconf skipped SDN ${sdn_index}"; exit 0; }
private="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
case "$config" in
    /etc/dnsmasq.conf|/tmp/etc/dnsmasq.conf) ;;
    *) [ "${SOCKROCKET_DNS_MAIN_CONFIG:-}" = "$config" ] || {
        postconf_log "Merlin 3006 DNS postconf skipped ${config}"
        exit 0
    } ;;
esac
# An empty file is rc still generating the config. Leave it untouched.
if [ ! -s "$config" ] || [ ! -s "$private" ]; then
    postconf_log "Merlin 3006 DNS postconf skipped ${config}: config or upstream empty"
    exit 1
fi
upstream=$(sed -n 's/^\(server=127\.0\.0\.1#[0-9][0-9]*\)$/\1/p' "$private" | head -n 1)
if [ -z "$upstream" ]; then
    postconf_log "Merlin 3006 DNS postconf skipped ${config}: upstream line missing"
    exit 1
fi
# Firmware postconf and the controller's repair may run concurrently. A
# shared temp name lets one delete the other's output; `cat > config` then
# truncates the live config before it discovers the missing input. Keep each
# output private and publish by rename so readers never see an empty file.
tmpdir="${config}.sockrocket-tmp.$$"
(umask 077; mkdir "$tmpdir") || exit 1
tmp="$tmpdir/config"
trap 'rm -f "$tmp"; rmdir "$tmpdir" 2>/dev/null || true' EXIT
trap 'exit 1' HUP INT TERM
# Drop every shared or ISP upstream. Keep domain overrides
# (server=/example.com/...). One no-resolv stops /etc/resolv.conf from
# racing Sockrocket. SDN configs are not edited.
awk -v upstream="$upstream" '
    /servers-file/ { next }
    /resolv-file/ { next }
    /^no-resolv$/ {
        if (!noresolv) print
        noresolv = 1
        next
    }
    $0 == upstream {
        if (!have) print
        have = 1
        next
    }
    /^server=\// { print; next }
    /^server=/ { next }
    { print }
    END {
        if (NR == 0) exit 1
        if (!noresolv) print "no-resolv"
        if (!have) print upstream
    }
' "$config" > "$tmp" || { rm -f "$tmp"; exit 1; }
if grep -q 'servers-file' "$tmp" || grep -q 'resolv-file' "$tmp" ||
   ! grep -qx 'no-resolv' "$tmp" || ! grep -Fx "$upstream" "$tmp" >/dev/null 2>&1; then
    rm -f "$tmp"
    postconf_log "Merlin 3006 DNS postconf failed ${config}"
    exit 1
fi
chmod 644 "$tmp" && mv -f "$tmp" "$config" || {
    postconf_log "Merlin 3006 DNS postconf failed ${config}"
    exit 1
}
postconf_log "Merlin 3006 DNS postconf isolated ${config}"
exit 0
