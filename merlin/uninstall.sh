#!/bin/sh
# Sockrocket Plugin Uninstaller
# Supports koolcenter software center and standard AsusWRT-Merlin

SOCKROCKET_DIR="/jffs/addons/sockrocket"
WWW_SOCKROCKET="/www/ext/sockrocket"
CGI_BIN="/www/cgi-bin"
MODULE="sockrocket"
KS_DIR="/koolshare"
KS_WEBS="$KS_DIR/webs"
KS_SCRIPTS="$KS_DIR/scripts"
KS_INIT="$KS_DIR/init.d"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info() { printf "${CYAN}[Sockrocket] %s${NC}\n" "$*"; }
ok()   { printf "${GREEN}[Sockrocket] ✓ %s${NC}\n" "$*"; }
warn() { printf "${YELLOW}[Sockrocket] ! %s${NC}\n" "$*"; }

# ── Detect koolshare environment ───────────────────────────────────────────────
KOOLSHARE=0
if [ -d "$KS_DIR" ] && [ -f "$KS_DIR/scripts/base.sh" ]; then
    # shellcheck source=/dev/null
    . "$KS_DIR/scripts/base.sh" 2>/dev/null || true
    KOOLSHARE=1
fi

# ── Confirm ───────────────────────────────────────────────────────────────────
# koolshare runs uninstall.sh non-interactively; skip prompt if invoked that way
if [ "$KOOLSHARE" != "1" ] && [ "$1" != "--force" ]; then
    printf "This will completely remove Sockrocket. Continue? [y/N] "
    read -r ans
    case "$ans" in y|Y) ;; *) echo "Aborted."; exit 0 ;; esac
fi

# ── Stop service ──────────────────────────────────────────────────────────────
info "Stopping Sockrocket service..."
[ -x "$SOCKROCKET_DIR/scripts/sockrocket.sh" ] && "$SOCKROCKET_DIR/scripts/sockrocket.sh" stop 2>/dev/null || true
# Stop the always-on Web UI API bridge as well (sockrocket.sh stop leaves it running)
[ -x "$SOCKROCKET_DIR/scripts/sockrocket.sh" ] && "$SOCKROCKET_DIR/scripts/sockrocket.sh" api-stop 2>/dev/null || true

# Kill any leftover sockrocket-cli processes
if [ -f "$SOCKROCKET_DIR/sockrocket.pid" ]; then
    pid=$(cat "$SOCKROCKET_DIR/sockrocket.pid" 2>/dev/null)
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
fi
if [ -f "$SOCKROCKET_DIR/sockrocket-api.pid" ]; then
    pid=$(cat "$SOCKROCKET_DIR/sockrocket-api.pid" 2>/dev/null)
    [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
fi

# ── Remove iptables rules ──────────────────────────────────────────────────────
info "Removing iptables rules..."
[ -x "$SOCKROCKET_DIR/scripts/iptables.sh" ] && "$SOCKROCKET_DIR/scripts/iptables.sh" stop 2>/dev/null || true

# ── Remove cron jobs ──────────────────────────────────────────────────────────
info "Removing cron jobs..."
cru d SockrocketSubUpdate 2>/dev/null || true
cru d SockrocketWatchdog  2>/dev/null || true
ok "Cron jobs removed"

# ── Remove Web UI ─────────────────────────────────────────────────────────────
info "Removing Web UI..."
rm -rf "$WWW_SOCKROCKET"
rm -f  "$CGI_BIN/sockrocket.cgi"
ok "Web UI removed"

# ── Remove startup hooks ───────────────────────────────────────────────────────
info "Removing startup hooks..."
remove_hook() {
    local script="$1"
    [ ! -f "$script" ] && return
    # Remove new-style and legacy anchor blocks
    sed -i '/# >>> SOCKROCKET_AUTO_START >>>/,/# <<< SOCKROCKET_AUTO_END <</d' "$script" 2>/dev/null || true
    # Also clean old-style hook comment lines
    sed -i '/^# Sockrocket \(firewall\|services\|service-event\)/d' "$script" 2>/dev/null || true
    sed -i '\|/jffs/addons/sockrocket/|d' "$script" 2>/dev/null || true
    # Clean up trailing empty lines left by block removal
    sed -i '/^$/{ N; /^\n$/d }' "$script" 2>/dev/null || true
    ok "Cleaned $(basename "$script")"
}

remove_hook /jffs/scripts/firewall-start
remove_hook /jffs/scripts/services-start
remove_hook /jffs/scripts/service-event
remove_hook /jffs/scripts/wan-start

# ── Remove dnsmasq config + upstream pin leftovers ────────────────────────────
info "Removing dnsmasq config..."
rm -f /jffs/configs/dnsmasq.d/sockrocket.conf
# stop already unpins; belt-and-braces if stop was skipped / failed.
rm -f /tmp/resolv.dnsmasq.Sockrocket-bak
if grep -q "^server=127.0.0.1#" /tmp/resolv.dnsmasq 2>/dev/null; then
    # Rebuild from system resolver so LAN DNS is not left on a dead listener.
    awk '/^nameserver/ && $2 != "127.0.0.1" && $2 != "::1" {print "server=" $2}' \
        /etc/resolv.conf 2>/dev/null > /tmp/resolv.dnsmasq.new \
        && [ -s /tmp/resolv.dnsmasq.new ] \
        && mv /tmp/resolv.dnsmasq.new /tmp/resolv.dnsmasq \
        || rm -f /tmp/resolv.dnsmasq.new
fi
service restart_dnsmasq 2>/dev/null || true
ok "dnsmasq config removed"

# ── koolshare-specific cleanup ────────────────────────────────────────────────
if [ "$KOOLSHARE" = "1" ]; then
    info "Removing koolshare integration..."
    rm -f "$KS_WEBS/Module_sockrocket.asp"
    rm -f "$KS_DIR/res/icon-sockrocket.png"
    rm -f "$KS_SCRIPTS/${MODULE}_config.sh"
    rm -f "$KS_SCRIPTS/${MODULE}_api.sh"
    rm -f "$KS_INIT/S98${MODULE}.sh"
    # Deregister from software center
    dbus remove "softcenter_module_${MODULE}_install"    2>/dev/null || true
    dbus remove "softcenter_module_${MODULE}_version"    2>/dev/null || true
    dbus remove "softcenter_module_${MODULE}_name"       2>/dev/null || true
    dbus remove "softcenter_module_${MODULE}_title"      2>/dev/null || true
    dbus remove "softcenter_module_${MODULE}_description" 2>/dev/null || true
    dbus remove "softcenter_module_${MODULE}_indexpage"   2>/dev/null || true
    dbus remove "${MODULE}_version"                      2>/dev/null || true
    ok "koolshare integration removed"
fi

# ── Remove Sockrocket files + tmp leftovers ───────────────────────────────────
info "Removing Sockrocket files..."
rm -rf "$SOCKROCKET_DIR"
rm -f /tmp/sockrocket-core.* /tmp/sockrocket_health.json 2>/dev/null || true
rm -f /tmp/upload/sockrocket_rpc_*.json 2>/dev/null || true
ok "Sockrocket files removed"

# ── Verify cleanup ────────────────────────────────────────────────────────────
verify_cleanup() {
    local leftover=0

    check_leftover() {
        if [ -e "$1" ]; then
            warn "  LEFTOVER: $1"
            leftover=$((leftover + 1))
        fi
    }

    info "Checking for leftover artifacts..."
    check_leftover "$SOCKROCKET_DIR"
    check_leftover "$WWW_SOCKROCKET"
    check_leftover "$CGI_BIN/sockrocket.cgi"
    check_leftover "/koolshare/webs/Module_sockrocket.asp"

    # Check hooks still contain Sockrocket markers
    for script in /jffs/scripts/firewall-start /jffs/scripts/services-start /jffs/scripts/service-event /jffs/scripts/wan-start; do
        if [ -f "$script" ] && grep -qE "SOCKROCKET_AUTO_START|/jffs/addons/sockrocket/" "$script" 2>/dev/null; then
            warn "  LEFTOVER: Sockrocket markers in $(basename "$script")"
            leftover=$((leftover + 1))
        fi
    done
    check_leftover "/koolshare/scripts/sockrocket_api.sh"

    if [ "$leftover" -eq 0 ]; then
        ok "Cleanup complete, no leftovers"
    else
        warn "Cleanup done, but $leftover artifact(s) remain (manual cleanup may be needed)"
    fi
}

verify_cleanup

echo ""
ok "Sockrocket uninstalled successfully."
