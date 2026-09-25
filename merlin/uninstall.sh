#!/bin/sh
# Sockrocket Plugin Uninstaller
# Supports koolcenter software center and standard AsusWRT-Merlin.
#
# Softcenter looks for: /koolshare/scripts/uninstall_sockrocket.sh
# (install.sh must install this path; otherwise softcenter only clears dbus
# and leaves /jffs/addons/sockrocket behind.)

SOCKROCKET_DIR="/jffs/addons/sockrocket"
WWW_SOCKROCKET="/www/ext/sockrocket"
CGI_BIN="/www/cgi-bin"
MODULE="sockrocket"
KS_DIR="/koolshare"
KS_WEBS="$KS_DIR/webs"
KS_SCRIPTS="$KS_DIR/scripts"
KS_INIT="$KS_DIR/init.d"
KS_UNINSTALL="$KS_SCRIPTS/uninstall_${MODULE}.sh"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info() { printf "${CYAN}[Sockrocket] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Sockrocket] ✓ %s${NC}\n" "$*"; }
warn() { printf "${YELLOW}[Sockrocket] ! %s${NC}\n" "$*"; }

# Softcenter / non-interactive: never prompt
NONINTERACTIVE=0
case "$1" in --force|-f) NONINTERACTIVE=1 ;; esac
case "$(basename "$0" 2>/dev/null)" in
    uninstall_sockrocket.sh|uninstall_${MODULE}.sh) NONINTERACTIVE=1 ;;
esac
if [ -d "$KS_DIR" ] && [ -f "$KS_DIR/scripts/base.sh" ]; then
    NONINTERACTIVE=1
    # shellcheck source=/dev/null
    . "$KS_DIR/scripts/base.sh" 2>/dev/null || true
fi

if [ "$NONINTERACTIVE" != "1" ]; then
    printf "This will completely remove Sockrocket. Continue? [y/N] "
    read -r ans
    case "$ans" in y|Y) ;; *) echo "Aborted."; exit 0 ;; esac
fi

info "Uninstalling Sockrocket (full cleanup)..."

# ── Stop everything first ─────────────────────────────────────────────────────
info "Stopping services..."
[ -x "$SOCKROCKET_DIR/scripts/sockrocket.sh" ] && {
    "$SOCKROCKET_DIR/scripts/sockrocket.sh" stop 2>/dev/null || true
    "$SOCKROCKET_DIR/scripts/sockrocket.sh" api-stop 2>/dev/null || true
}
[ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && \
    "$SOCKROCKET_DIR/scripts/iptables.sh" stop 2>/dev/null || true

# PID files
for pf in "$SOCKROCKET_DIR/sockrocket.pid" "$SOCKROCKET_DIR/sockrocket-api.pid"; do
    if [ -f "$pf" ]; then
        pid=$(cat "$pf" 2>/dev/null)
        [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true
    fi
done

# Any leftover binary (api + daemon)
killall sockrocket-cli 2>/dev/null || true
killall -9 sockrocket-cli 2>/dev/null || true
sleep 1

# ── Cron ──────────────────────────────────────────────────────────────────────
info "Removing cron jobs..."
cru d SockrocketSubUpdate 2>/dev/null || true
cru d SockrocketWatchdog  2>/dev/null || true

# ── Startup hooks ─────────────────────────────────────────────────────────────
info "Removing startup hooks..."
remove_hook() {
    local script="$1"
    [ ! -f "$script" ] && return 0
    sed -i '/# >>> SOCKROCKET_AUTO_START >>>/,/# <<< SOCKROCKET_AUTO_END <</d' "$script" 2>/dev/null || true
    sed -i '/^# Sockrocket \(firewall\|services\|service-event\|wan\)/d' "$script" 2>/dev/null || true
    sed -i '\|/jffs/addons/sockrocket/|d' "$script" 2>/dev/null || true
    sed -i '/sockrocket/d' "$script" 2>/dev/null || true
}
remove_hook /jffs/scripts/firewall-start
remove_hook /jffs/scripts/services-start
remove_hook /jffs/scripts/service-event
remove_hook /jffs/scripts/wan-start
remove_hook /jffs/scripts/nat-start
ok "Hooks cleaned"

# ── dnsmasq / DNS pin ─────────────────────────────────────────────────────────
info "Restoring DNS..."
rm -f /jffs/configs/dnsmasq.d/sockrocket.conf
rm -f /tmp/resolv.dnsmasq.Sockrocket-bak
if [ -f /tmp/resolv.dnsmasq ] && grep -q "^server=127.0.0.1#" /tmp/resolv.dnsmasq 2>/dev/null; then
    awk '/^nameserver/ && $2 != "127.0.0.1" && $2 != "::1" {print "server=" $2}' \
        /etc/resolv.conf 2>/dev/null > /tmp/resolv.dnsmasq.new \
        && [ -s /tmp/resolv.dnsmasq.new ] \
        && mv /tmp/resolv.dnsmasq.new /tmp/resolv.dnsmasq \
        || rm -f /tmp/resolv.dnsmasq.new
fi
service restart_dnsmasq 2>/dev/null || killall -HUP dnsmasq 2>/dev/null || true

# ── koolshare / softcenter files ──────────────────────────────────────────────
info "Removing software-center files..."
rm -f "$KS_WEBS/Module_${MODULE}.asp" \
      "$KS_WEBS/Module_sockrocket.asp" \
      "$KS_DIR/res/icon-sockrocket.png" \
      "$KS_SCRIPTS/${MODULE}_config.sh" \
      "$KS_SCRIPTS/${MODULE}_api.sh" \
      "$KS_SCRIPTS/sockrocket_api.sh" \
      "$KS_SCRIPTS/sockrocket_config.sh" \
      "$KS_INIT/S98${MODULE}.sh" \
      "$KS_INIT/S99${MODULE}.sh" 2>/dev/null || true
# Drop broken symlink if init.d pointed at removed script
[ -L "$KS_INIT/S98${MODULE}.sh" ] && rm -f "$KS_INIT/S98${MODULE}.sh" 2>/dev/null || true

# ── dbus: wipe every sockrocket* key ──────────────────────────────────────────
info "Removing dbus keys..."
if which dbus >/dev/null 2>&1; then
    # Explicit softcenter module keys
    for k in \
        softcenter_module_${MODULE}_install \
        softcenter_module_${MODULE}_version \
        softcenter_module_${MODULE}_name \
        softcenter_module_${MODULE}_title \
        softcenter_module_${MODULE}_description \
        softcenter_module_${MODULE}_indexpage \
        softcenter_module_${MODULE}_home_url \
        ${MODULE}_version
    do
        dbus remove "$k" 2>/dev/null || true
    done
    # Sweep any remaining sockrocket* keys (rpc seq, toggles, etc.)
    dbus list 2>/dev/null | grep -i sockrocket | while IFS= read -r line; do
        k="${line%%=*}"
        [ -n "$k" ] && dbus remove "$k" 2>/dev/null || true
    done
fi
ok "dbus cleaned"

# ── Web UI / CGI ──────────────────────────────────────────────────────────────
info "Removing Web UI / CGI..."
rm -rf "$WWW_SOCKROCKET"
rm -f "$CGI_BIN/sockrocket.cgi" "$CGI_BIN/sockrocket" 2>/dev/null || true

# ── Plugin data directory (the big leftover) ──────────────────────────────────
info "Removing /jffs/addons/sockrocket ..."
rm -rf "$SOCKROCKET_DIR"
# Second pass in case a process recreated files
sleep 1
killall -9 sockrocket-cli 2>/dev/null || true
rm -rf "$SOCKROCKET_DIR"

# ── tmp leftovers ─────────────────────────────────────────────────────────────
info "Removing temp leftovers..."
rm -rf /tmp/sockrocket /tmp/sockrocket.* 2>/dev/null || true
rm -f /tmp/sockrocket* 2>/dev/null || true
rm -f /tmp/upload/sockrocket* 2>/dev/null || true
rm -f /tmp/sockrocket-merlin*.tar.gz /tmp/sockrocket*.tar.gz 2>/dev/null || true

# ── Verify + force-remove any leftovers ───────────────────────────────────────
force_rm() {
    [ -e "$1" ] || [ -L "$1" ] || return 0
    warn "Force-removing leftover: $1"
    rm -rf "$1" 2>/dev/null || true
}

info "Verifying cleanup..."
force_rm "$SOCKROCKET_DIR"
force_rm "$WWW_SOCKROCKET"
force_rm "$CGI_BIN/sockrocket.cgi"
force_rm "$KS_WEBS/Module_${MODULE}.asp"
force_rm "$KS_DIR/res/icon-sockrocket.png"
force_rm "$KS_SCRIPTS/sockrocket_api.sh"
force_rm "$KS_SCRIPTS/sockrocket_config.sh"
force_rm "$KS_SCRIPTS/${MODULE}_config.sh"
force_rm "$KS_SCRIPTS/${MODULE}_api.sh"
force_rm "$KS_INIT/S98${MODULE}.sh"
force_rm /jffs/configs/dnsmasq.d/sockrocket.conf

leftover=0
for p in \
    "$SOCKROCKET_DIR" \
    "$WWW_SOCKROCKET" \
    "$CGI_BIN/sockrocket.cgi" \
    "$KS_WEBS/Module_${MODULE}.asp" \
    "$KS_DIR/res/icon-sockrocket.png" \
    "$KS_SCRIPTS/sockrocket_api.sh" \
    "$KS_SCRIPTS/sockrocket_config.sh"
do
    if [ -e "$p" ] || [ -L "$p" ]; then
        warn "STILL PRESENT: $p"
        leftover=$((leftover + 1))
    fi
done

for script in /jffs/scripts/firewall-start /jffs/scripts/services-start \
              /jffs/scripts/service-event /jffs/scripts/wan-start /jffs/scripts/nat-start; do
    if [ -f "$script" ] && grep -qi sockrocket "$script" 2>/dev/null; then
        warn "STILL PRESENT: sockrocket refs in $script"
        sed -i '/sockrocket/d' "$script" 2>/dev/null || true
        leftover=$((leftover + 1))
    fi
done

# Self-remove softcenter uninstall wrapper last (ks_app_remove also deletes it)
if [ -f "$KS_UNINSTALL" ]; then
    # If we are that file, schedule delete after exit
    if [ "$(basename "$0")" = "uninstall_${MODULE}.sh" ] || \
       [ "$0" = "$KS_UNINSTALL" ]; then
        ( sleep 1; rm -f "$KS_UNINSTALL" ) >/dev/null 2>&1 &
    else
        rm -f "$KS_UNINSTALL" 2>/dev/null || true
    fi
fi

sync

if [ "$leftover" -eq 0 ]; then
    ok "Cleanup complete — no leftovers"
else
    warn "Cleanup finished with $leftover warning(s); re-check paths above"
fi

echo ""
ok "Sockrocket uninstalled successfully."
exit 0
