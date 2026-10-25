#!/bin/sh
# Keep the main LAN's proxy upstream separate from Merlin 3006 SDN dnsmasq
# instances, which can read the firmware's shared /tmp/resolv.dnsmasq.
config="$1"
private="${SOCKROCKET_DNS_UPSTREAM_FILE:-/tmp/sockrocket-resolv.dnsmasq}"
case "$config" in /etc/dnsmasq.conf) ;; *) [ "${SOCKROCKET_DNS_MAIN_CONFIG:-}" = "$config" ] || exit 1 ;; esac
[ -f "$config" ] && [ -f "$private" ] || exit 1
tmp="${config}.sockrocket-tmp"
sed "s|^servers-file=/tmp/resolv.dnsmasq$|servers-file=$private|" "$config" > "$tmp" || exit 1
if ! grep -Fx "servers-file=$private" "$tmp" >/dev/null 2>&1; then
    rm -f "$tmp"
    exit 1
fi
cat "$tmp" > "$config" || { rm -f "$tmp"; exit 1; }
rm -f "$tmp"
