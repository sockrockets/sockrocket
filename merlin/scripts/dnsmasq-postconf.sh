#!/bin/sh
# Point only the main LAN dnsmasq at Sockrocket's private upstream.
# Merlin 3006 SDN instances use their own config and dnsmasq-sdn.postconf.
# They keep the firmware resolv file, including a guest network with its own DNS.
config="$1"
private="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
case "$config" in
    /etc/dnsmasq.conf) ;;
    *) [ "${SOCKROCKET_DNS_MAIN_CONFIG:-}" = "$config" ] || exit 0 ;;
esac
[ -f "$config" ] && [ -f "$private" ] || exit 1
tmp="${config}.sockrocket-tmp"
# Stock Merlin writes servers-file=/tmp/resolv.dnsmasq. A fork may use another
# path; rewrite whatever servers-file the main config has. Drop other global
# server= lines only when that rewrite is possible, so a faster ISP, DNSFilter
# or Stubby answer cannot beat Sockrocket. Domain overrides
# (server=/example.com/...) stay. SDN configs are not edited.
awk -v private="$private" '
    /^servers-file=/ { print "servers-file=" private; found = 1; next }
    /^server=\// { print; next }
    /^server=/ { next }
    { print }
    END { if (!found) exit 1 }
' "$config" > "$tmp" || { rm -f "$tmp"; exit 1; }
if ! grep -Fx "servers-file=${private}" "$tmp" >/dev/null 2>&1; then
    rm -f "$tmp"
    exit 1
fi
cat "$tmp" > "$config" || { rm -f "$tmp"; exit 1; }
rm -f "$tmp"
exit 0
