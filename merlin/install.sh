#!/bin/sh
# Sockrocket Plugin Installer
# Supports:
#   1. koolcenter / koolshare software center (auto-detected)
#   2. Standard AsusWRT-Merlin firmware (armv7 / aarch64)
#
# Usage: sh install.sh [arch|platform]   — offline install from bundled package
#        sh install.sh --update [arch]   — update binary only (preserve config)
#   arch:     armv7 | aarch64 (CPU; auto-detected if not specified)
#   platform: arm | hnd | hnd_v8 | qca | mtk | ipq32 | ipq64
#             (koolcenter platform tags; same as fancyss package naming)
#   Fully offline: this script never downloads anything.
#
# This script assumes NOTHING about the host toolchain. koolcenter's offline
# installer runs package scripts in a stripped environment (no busybox, no
# sed/grep/awk, restricted PATH); every external command is probed first and
# has a pure-shell or sockrocket-cli fallback, and missing runtime tools (iptables,
# nvram, cru, …) only produce warnings — runtime wiring is completed by the
# services-start hook in the real firmware environment.

# Do NOT use set -e — koolshare's base.sh changes error handling

# ── Common paths ──────────────────────────────────────────────────────────────
# Plugin data always lives under /jffs/addons/sockrocket/ (both modes)
SOCKROCKET_DIR="/jffs/addons/sockrocket"
SOCKROCKET_BIN="$SOCKROCKET_DIR/sockrocket-cli"
SOCKROCKET_CONF="$SOCKROCKET_DIR/config.yaml"
SOCKROCKET_SCRIPTS="$SOCKROCKET_DIR/scripts"
CGI_BIN="/www/cgi-bin"

# koolshare paths
KS_DIR="/koolshare"
KS_WEBS="$KS_DIR/webs"
KS_SCRIPTS="$KS_DIR/scripts"
KS_INIT="$KS_DIR/init.d"
MODULE="sockrocket"   # must match the package directory name

# Standard Merlin path (used when NOT in koolshare mode)
WWW_SOCKROCKET="/www/ext/sockrocket"

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()  { which logger >/dev/null 2>&1 && logger -t sockrocket-install -p daemon.info "$*";  printf "${CYAN}[Sockrocket] %s${NC}\n" "$*"; }
ok()    { which logger >/dev/null 2>&1 && logger -t sockrocket-install -p daemon.info "$*";  printf "${GREEN}[Sockrocket] ✓ %s${NC}\n" "$*"; }
warn()  { which logger >/dev/null 2>&1 && logger -t sockrocket-install -p daemon.warn "$*";  printf "${YELLOW}[Sockrocket] ! %s${NC}\n" "$*" >&2; }
err()   { which logger >/dev/null 2>&1 && logger -t sockrocket-install -p daemon.err "$*";   printf "${RED}[Sockrocket] ✗ %s${NC}\n" "$*" >&2; exit 1; }
die()   { which logger >/dev/null 2>&1 && logger -t sockrocket-install -p daemon.crit "$*";  printf "${RED}[Sockrocket] FATAL: %s${NC}\n" "$*" >&2; exit 1; }

# ── Minimal toolbelt (probed, with fallbacks) ──────────────────────────────────
have() { which "$1" >/dev/null 2>&1; }

# Restore the router-standard PATH (koolcenter strips it, base.sh clobbers it).
restore_path() {
    for d in /usr/sbin /usr/bin /sbin /bin /opt/sbin /opt/bin /koolshare/bin /koolshare/scripts; do
        [ -d "$d" ] && case ":$PATH:" in *":$d:"*) ;; *) PATH="$d:$PATH" ;; esac
    done
    export PATH
}
restore_path

# xcp: cp with a pure-shell fallback (cat > dst)
xcp() {
    if have cp; then
        cp "$1" "$2"
    else
        cat < "$1" > "$2"
    fi
}

# xlink: ln -sf with a copy fallback
xlink() {
    if have ln; then
        ln -sf "$1" "$2" 2>/dev/null && return 0
    fi
    xcp "$1" "$2" 2>/dev/null
}

# read the first line of a file without cat
read_first_line() {
    IFS= read -r _rfl < "$1" 2>/dev/null || _rfl=""
    printf '%s' "$_rfl"
}

# ── Detect koolshare environment ───────────────────────────────────────────────
# NOTE: deliberately NOT sourcing base.sh — we use none of its helpers, and it
# is free to clobber PATH or redefine shell helpers (which once broke our arch
# detection). File existence is enough to detect the environment.
KOOLSHARE=0
if [ -d "$KS_DIR" ] && [ -f "$KS_DIR/scripts/base.sh" ]; then
    KOOLSHARE=1
fi
restore_path

# ── Parse arguments ────────────────────────────────────────────────────────────
UPDATE_ONLY=false
ARCH_ARG=""
for arg in "$@"; do
    case "$arg" in
        --update) UPDATE_ONLY=true ;;
        armv7|aarch64) ARCH_ARG="$arg" ;;
        # fancyss-style platform tags map to CPU arch for binary selection
        arm|hnd|qca|ipq32) ARCH_ARG="armv7" ;;
        hnd_v8|mtk|ipq64)  ARCH_ARG="aarch64" ;;
    esac
done

# ── Architecture detection ─────────────────────────────────────────────────────
# No command -v guard here (base.sh may have redefined our helpers in earlier
# revisions): call uname directly, fall back to absolute paths, then cpuinfo.
detect_arch() {
    local machine=""
    machine=$(uname -m 2>/dev/null) || machine=""
    [ -z "$machine" ] && [ -x /usr/bin/uname ] && machine=$(/usr/bin/uname -m 2>/dev/null)
    [ -z "$machine" ] && [ -x /bin/uname ] && machine=$(/bin/uname -m 2>/dev/null)
    if [ -z "$machine" ] && [ -r /proc/cpuinfo ]; then
        # Pure-shell fallback: scan cpuinfo for architecture markers
        local line
        while IFS= read -r line; do
            case "$line" in
                *aarch64*|*AArch64*) machine="aarch64"; break ;;
                *ARMv7*|*armv7*)     machine="armv7"; break ;;
            esac
        done < /proc/cpuinfo
    fi
    case "$machine" in
        armv7*|armhf)  echo "armv7" ;;
        aarch64|arm64) echo "aarch64" ;;
        *) echo "" ;;
    esac
}

SCRIPT_DIR="$(cd "$(dirname "${0}")" 2>/dev/null && pwd)"
[ -z "$SCRIPT_DIR" ] && SCRIPT_DIR="/tmp/$MODULE"

if [ -n "$ARCH_ARG" ]; then
    ARCH="$ARCH_ARG"
else
    ARCH="$(detect_arch)"
fi
if [ -z "$ARCH" ]; then
    warn "PATH=$PATH"
    err "Cannot auto-detect architecture, please specify manually: sh install.sh aarch64 or sh install.sh armv7"
fi
info "Target architecture: $ARCH"

# ── Dependency report (informational only — never fatal) ──────────────────────
check_deps() {
    restore_path
    local missing=""
    for cmd in cp chmod ln mkdir rm sed grep base64 awk pgrep curl iptables ip service nvram cru dbus; do
        have "$cmd" || missing="$missing $cmd"
    done
    if [ -n "$missing" ]; then
        warn "Restricted install environment, the following tools are unavailable:$missing"
        warn "Falling back to plain shell; runtime wiring is completed by the services-start hook in a normal environment"
    else
        info "All system dependencies found"
    fi
}

# ── Resolve binary source (offline only — never downloads anything) ───────────
# Prefer a single bin/sockrocket-cli (fancyss-style per-platform package), then
# fall back to arch-suffixed names used by older / multi-arch bundles.
BINARY=""
if [ -f "$SCRIPT_DIR/bin/sockrocket-cli" ]; then
    BINARY="$SCRIPT_DIR/bin/sockrocket-cli"
    info "Using bundled binary: $BINARY"
elif [ -f "$SCRIPT_DIR/bin/sockrocket-cli-$ARCH" ]; then
    BINARY="$SCRIPT_DIR/bin/sockrocket-cli-$ARCH"
    info "Using bundled binary: $BINARY"
else
    err "Built-in binary not found under $SCRIPT_DIR/bin/
  Expected: bin/sockrocket-cli  or  bin/sockrocket-cli-$ARCH
  Use the offline package matching your router platform
  (sockrocket-merlin-{arm|hnd|hnd_v8|qca|mtk|ipq32|ipq64}.tar.gz).
  If architecture detection was wrong, specify manually:
    sh install.sh aarch64 | sh install.sh armv7
    sh install.sh hnd_v8  | sh install.sh hnd"
fi

# ── Environment check ─────────────────────────────────────────────────────────
JFFS_OK=0
have mountpoint && mountpoint -q /jffs 2>/dev/null && JFFS_OK=1
have df && df /jffs >/dev/null 2>&1 && JFFS_OK=1
[ ! -d /jffs ] || JFFS_OK=1   # directory existing is good enough in practice
if [ "$JFFS_OK" != "1" ]; then
    die "/jffs is not mounted. Enable JFFS partition in router web UI (Administration -> System -> Enable JFFS custom scripts)"
fi
info "/jffs partition OK"
check_deps

# ── Update-only mode ───────────────────────────────────────────────────────────
if [ "$UPDATE_ONLY" = "true" ]; then
    info "Update mode: replacing binary only (config preserved)..."
    [ -d "$SOCKROCKET_DIR" ] || err "Sockrocket is not installed. Run without --update first."
    WAS_RUNNING=false
    PID="$(read_first_line "$SOCKROCKET_DIR/sockrocket.pid")"
    [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null && WAS_RUNNING=true
    [ "$WAS_RUNNING" = "true" ] && "$SOCKROCKET_SCRIPTS/sockrocket.sh" stop 2>/dev/null || true
    xcp "$BINARY" "$SOCKROCKET_BIN" && chmod +x "$SOCKROCKET_BIN" 2>/dev/null
    ok "Binary updated: $("$SOCKROCKET_BIN" --version 2>/dev/null || echo 'version unknown')"
    # Restart the API bridge too if it is running — it still holds the old binary
    [ -f "$SOCKROCKET_DIR/sockrocket-api.pid" ] && "$SOCKROCKET_SCRIPTS/sockrocket.sh" api-restart 2>/dev/null || true
    [ "$WAS_RUNNING" = "true" ] && "$SOCKROCKET_SCRIPTS/sockrocket.sh" start
    exit 0
fi

# ── Shared helpers ─────────────────────────────────────────────────────────────
install_common_files() {
    # Binary, scripts, config — same in both modes
    info "Creating data directories..."
    mkdir -p "$SOCKROCKET_DIR" "$SOCKROCKET_SCRIPTS" "$CGI_BIN" /jffs/scripts /jffs/configs/dnsmasq.d 2>/dev/null

    if [ -f "$SOCKROCKET_CONF" ]; then
        xcp "$SOCKROCKET_CONF" "$SOCKROCKET_CONF.bak"
        ok "Existing config backed up to $SOCKROCKET_CONF.bak"
    fi

    info "Installing sockrocket-cli binary..."
    xcp "$BINARY" "$SOCKROCKET_BIN" && chmod +x "$SOCKROCKET_BIN" 2>/dev/null
    ok "Binary installed: $("$SOCKROCKET_BIN" --version 2>/dev/null || echo 'version unknown')"

    info "Installing control scripts..."
    xcp "$SCRIPT_DIR/scripts/sockrocket.sh"       "$SOCKROCKET_SCRIPTS/sockrocket.sh"
    xcp "$SCRIPT_DIR/scripts/iptables.sh"  "$SOCKROCKET_SCRIPTS/iptables.sh"
    xcp "$SCRIPT_DIR/scripts/dnsmasq.conf" "$SOCKROCKET_SCRIPTS/dnsmasq.conf.template"
    [ -f "$SCRIPT_DIR/scripts/sockrocket_api.sh" ] && xcp "$SCRIPT_DIR/scripts/sockrocket_api.sh" "$SOCKROCKET_SCRIPTS/sockrocket_api.sh"
    mkdir -p "$SOCKROCKET_DIR/webui" 2>/dev/null
    xcp "$SCRIPT_DIR/webui/sockrocket.asp" "$SOCKROCKET_DIR/webui/sockrocket.asp" 2>/dev/null || true
    if [ -f "$SCRIPT_DIR/res/icon-sockrocket.png" ]; then
        mkdir -p "$SOCKROCKET_DIR/res" 2>/dev/null || true
        xcp "$SCRIPT_DIR/res/icon-sockrocket.png" "$SOCKROCKET_DIR/res/icon-sockrocket.png" 2>/dev/null || true
    fi
    [ -f "$SCRIPT_DIR/version" ] && xcp "$SCRIPT_DIR/version" "$SOCKROCKET_DIR/version" 2>/dev/null || true
    # Keep uninstall next to the install for SSH / manual removal
    if [ -f "$SCRIPT_DIR/uninstall.sh" ]; then
        xcp "$SCRIPT_DIR/uninstall.sh" "$SOCKROCKET_DIR/uninstall.sh"
        chmod +x "$SOCKROCKET_DIR/uninstall.sh" 2>/dev/null || true
    fi
    chmod +x "$SOCKROCKET_SCRIPTS/sockrocket.sh" "$SOCKROCKET_SCRIPTS/iptables.sh" "$SOCKROCKET_SCRIPTS/sockrocket_api.sh" 2>/dev/null
    ok "Control scripts installed"

    info "Installing CGI backend..."
    # The CGI backend is built into sockrocket-cli itself (Rust, `cgi` module):
    # sockrocket.cgi is just a symlink to the binary, which detects argv[0] and
    # answers the web UI's JSON API calls directly. No shell CGI anymore.
    xlink "$SOCKROCKET_BIN" "$SOCKROCKET_DIR/sockrocket.cgi"
    mkdir -p "$CGI_BIN" 2>/dev/null
    xlink "$SOCKROCKET_DIR/sockrocket.cgi" "$CGI_BIN/sockrocket.cgi" || \
        warn "Could not link CGI into $CGI_BIN — will retry on next start"
    ok "CGI backend installed (Rust, built into sockrocket-cli)"

    # Smoke-test the CGI binary (no grep — shell case match)
    CGI_OUT=$(QUERY_STRING="action=version" "$SOCKROCKET_DIR/sockrocket.cgi" 2>/dev/null)
    case "$CGI_OUT" in
        *'"version"'*) ok "CGI backend verified" ;;
        *)             warn "CGI smoke test failed — Web UI may not work" ;;
    esac

    if [ ! -f "$SOCKROCKET_CONF" ]; then
        info "Generating default config..."
        xcp "$SCRIPT_DIR/config.yaml.template" "$SOCKROCKET_CONF"
        ok "Default config at $SOCKROCKET_CONF"
    else
        ok "Existing config preserved"
    fi
}

# install_hook <script> <content...> — pure-shell, no sed/grep/awk.
install_hook() {
    local script="$1"
    shift 2>/dev/null  # discard script path (and legacy marker arg if present)
    local content="$*"

    # Create script if missing
    if [ ! -f "$script" ]; then
        printf '#!/bin/sh\n' > "$script" && chmod +x "$script" 2>/dev/null
    fi

    # Remove any previous Sockrocket block (anchor markers, old-style markers, and
    # any line referencing /jffs/addons/sockrocket/) using only shell builtins.
    local tmp="$script.Sockrocket-tmp"
    local in_block=0
    : > "$tmp"
    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            "# >>> SOCKROCKET_AUTO_START >>>") in_block=1; continue ;;
            "# <<< SOCKROCKET_AUTO_END <<<")   in_block=0; continue ;;
        esac
        [ "$in_block" = "1" ] && continue
        case "$line" in
            *"/jffs/addons/sockrocket/"*|*" /jffs/addons/sockrocket/"*|*"\"/jffs/addons/sockrocket/"*) continue ;;
            "# Sockrocket firewall"* | "# Sockrocket services"* | "# Sockrocket service-event"*) continue ;;
        esac
        printf '%s\n' "$line" >> "$tmp"
    done < "$script"
    # Copy back without relying on mv
    cat < "$tmp" > "$script" 2>/dev/null || xcp "$tmp" "$script"
    rm -f "$tmp" 2>/dev/null

    # Append with clear anchor markers
    {
        echo "# >>> SOCKROCKET_AUTO_START >>>"
        echo "# Auto-generated by Sockrocket install.sh. Do not edit between markers."
        echo "$content"
        echo "# <<< SOCKROCKET_AUTO_END <<<"
    } >> "$script"

    chmod +x "$script" 2>/dev/null
    ok "Hook installed in $(basename "$script" 2>/dev/null || echo "$script")"
}

# ─────────────────────────────────────────────────────────────────────────────
# koolshare / koolcenter install path
# ─────────────────────────────────────────────────────────────────────────────
ks_install() {
    local VER
    VER=$(read_first_line "$SCRIPT_DIR/version")
    [ -z "$VER" ] && VER="0.1.0"
    info "Installing Sockrocket ${VER} in koolcenter mode..."

    install_common_files

    # koolshare Web UI: Module_sockrocket.asp → /koolshare/webs/
    info "Installing koolshare Web UI..."
    mkdir -p "$KS_WEBS" "$KS_SCRIPTS" "$KS_INIT" "$KS_DIR/res" 2>/dev/null
    xcp "$SCRIPT_DIR/webui/sockrocket.asp" "$KS_WEBS/Module_sockrocket.asp"
    ok "Web UI installed to $KS_WEBS/Module_sockrocket.asp"

    # Software-center tile icon (softcenter loads /koolshare/res/icon-<module>.png)
    if [ -f "$SCRIPT_DIR/res/icon-sockrocket.png" ]; then
        xcp "$SCRIPT_DIR/res/icon-sockrocket.png" "$KS_DIR/res/icon-sockrocket.png"
        ok "Software-center icon installed"
    elif [ -f "$SCRIPT_DIR/webui/icon-sockrocket.png" ]; then
        xcp "$SCRIPT_DIR/webui/icon-sockrocket.png" "$KS_DIR/res/icon-sockrocket.png"
        ok "Software-center icon installed"
    else
        warn "Software-center icon missing (res/icon-sockrocket.png) — tile may show blank"
    fi

    # koolshare startup wrapper: delegates to our sockrocket.sh.
    # Written with printf, not a `cat <<EOF` heredoc: the koolcenter offline
    # installer's sandbox may not provide cat (that is why xcp exists), and
    # this write previously had no fallback — the whole wrapper was lost.
    printf '%s\n' \
        '#!/bin/sh' \
        'case "$1" in' \
        '    start|"") /jffs/addons/sockrocket/scripts/sockrocket.sh api-start' \
        '              /jffs/addons/sockrocket/scripts/sockrocket.sh start ;;' \
        '    stop)     /jffs/addons/sockrocket/scripts/sockrocket.sh stop' \
        '              /jffs/addons/sockrocket/scripts/sockrocket.sh api-stop ;;' \
        '    restart)  /jffs/addons/sockrocket/scripts/sockrocket.sh restart ;;' \
        '    *)        /jffs/addons/sockrocket/scripts/sockrocket.sh "$@" ;;' \
        'esac' > "$KS_SCRIPTS/${MODULE}_config.sh"
    chmod +x "$KS_SCRIPTS/${MODULE}_config.sh" 2>/dev/null

    # koolcenter API bridge script (platform job dispatcher target:
    # the page calls POST /_api/ {method: "sockrocket_api.sh"}, this script runs
    # the Rust CGI synchronously and drops the JSON into /tmp/upload/ for
    # the page to poll via /_temp/). /www is read-only on this platform,
    # so the /cgi-bin symlink path cannot work here.
    xcp "$SCRIPT_DIR/scripts/sockrocket_api.sh" "$KS_SCRIPTS/sockrocket_api.sh"
    chmod +x "$KS_SCRIPTS/sockrocket_api.sh" 2>/dev/null
    ok "API bridge installed to $KS_SCRIPTS/sockrocket_api.sh"

    # Softcenter looks for /koolshare/scripts/uninstall_<module>.sh
    # Without this file it only clears dbus and leaves /jffs/addons/sockrocket.
    if [ -f "$SCRIPT_DIR/uninstall.sh" ]; then
        xcp "$SCRIPT_DIR/uninstall.sh" "$KS_SCRIPTS/uninstall_${MODULE}.sh"
        chmod +x "$KS_SCRIPTS/uninstall_${MODULE}.sh" 2>/dev/null || true
        ok "Softcenter uninstall script installed (uninstall_${MODULE}.sh)"
    else
        warn "uninstall.sh missing from package — softcenter uninstall will be incomplete"
    fi

    # Register with koolshare init.d for auto-start
    xlink "$KS_SCRIPTS/${MODULE}_config.sh" "$KS_INIT/S98${MODULE}.sh"
    ok "Registered koolshare startup hook"

    # Register plugin metadata in dbus (software center status)
    if have dbus; then
        dbus set "${MODULE}_version"="${VER}"                        2>/dev/null || true
        dbus set "softcenter_module_${MODULE}_version"="${VER}"      2>/dev/null || true
        dbus set "softcenter_module_${MODULE}_install"="1"          2>/dev/null || true
        dbus set "softcenter_module_${MODULE}_name"="${MODULE}"      2>/dev/null || true
        dbus set "softcenter_module_${MODULE}_title"="Sockrocket"          2>/dev/null || true
        dbus set "softcenter_module_${MODULE}_description"="Transparent proxy (TUN/SOCKS5/HTTP)" 2>/dev/null || true
        # Page the software center opens when the icon is clicked. Without
        # indexpage the icon shows up but clicking it does nothing — the
        # plugin appears "unreachable" even though everything else works.
        dbus set "softcenter_module_${MODULE}_indexpage"="Module_${MODULE}.asp" 2>/dev/null || true
        ok "Plugin registered in dbus"
    else
        warn "dbus unavailable, skipping software-center status registration (no functional impact)"
    fi

    # koolshare installer expects temp dir cleanup
    rm -rf "/tmp/${MODULE}" "/tmp/${MODULE}.tar.gz" 2>/dev/null || true

    # Softcenter offline install does not run init.d hooks. Bring up the API
    # bridge (and SOCKS-only daemon) so the Web UI works immediately — without
    # this the page falls back to slow KSC polling and looks "stuck".
    if [ -x "$SOCKROCKET_SCRIPTS/sockrocket.sh" ]; then
        info "Starting API bridge and service..."
        "$SOCKROCKET_SCRIPTS/sockrocket.sh" api-start >>"$SOCKROCKET_DIR/sockrocket.log" 2>&1 || true
        "$SOCKROCKET_SCRIPTS/sockrocket.sh" start >>"$SOCKROCKET_DIR/sockrocket.log" 2>&1 || true
    fi

    echo ""
    ok "Sockrocket ${VER} installed successfully!"
    echo ""
    echo "  Config file : $SOCKROCKET_CONF"
    echo "  Web UI   : Software Center -> Sockrocket"
    echo ""
    echo "  Start: $SOCKROCKET_SCRIPTS/sockrocket.sh start"
    echo "  Stop: $SOCKROCKET_SCRIPTS/sockrocket.sh stop"
    echo ""
}

# ─────────────────────────────────────────────────────────────────────────────
# Standard AsusWRT-Merlin install path
# ─────────────────────────────────────────────────────────────────────────────
merlin_install() {
    info "Installing in Standard Merlin mode..."
    mkdir -p "$WWW_SOCKROCKET" 2>/dev/null
    install_common_files

    info "Installing Web UI page..."
    xcp "$SCRIPT_DIR/webui/sockrocket.asp" "$WWW_SOCKROCKET/sockrocket.asp"
    ok "Web UI installed to $WWW_SOCKROCKET/sockrocket.asp"

    info "Hooking into Merlin startup scripts..."

    install_hook "/jffs/scripts/firewall-start" \
'[ -f /jffs/addons/sockrocket/stopped ] && exit 0
# Reapply transparent proxy (mangle) when TUN is up. DNS NAT is intentionally
# NOT flushed by iptables.sh start; restore it if sockrocket.conf says hijack.
[ -x /jffs/addons/sockrocket/scripts/iptables.sh ] && /jffs/addons/sockrocket/scripts/iptables.sh start
[ -f /jffs/configs/dnsmasq.d/sockrocket.conf ] && \
  [ -x /jffs/addons/sockrocket/scripts/iptables.sh ] && \
  /jffs/addons/sockrocket/scripts/iptables.sh dns-start
'

    install_hook "/jffs/scripts/services-start" \
"[ -x /jffs/addons/sockrocket/scripts/sockrocket.sh ] && /jffs/addons/sockrocket/scripts/sockrocket.sh api-start
[ -x /jffs/addons/sockrocket/scripts/sockrocket.sh ] && /jffs/addons/sockrocket/scripts/sockrocket.sh start
"

    # WAN (re)configuration regenerates /tmp/resolv.dnsmasq with the ISP DNS
    # servers, undoing the hijack's upstream pin — re-pin after every WAN up.
    install_hook "/jffs/scripts/wan-start" \
'[ -x /jffs/addons/sockrocket/scripts/sockrocket.sh ] && /jffs/addons/sockrocket/scripts/sockrocket.sh pin-dns
'

    install_hook "/jffs/scripts/service-event" \
'if [ "$1" = "restart" ] && [ "$2" = "Sockrocket" ]; then
    /jffs/addons/sockrocket/scripts/sockrocket.sh restart
fi
'


    # Post-install validation
    info "Validating installation..."
    errors=0
    [ -x "$SOCKROCKET_BIN" ]                  || { warn "Binary not executable";       errors=$((errors+1)); }
    [ -f "$SOCKROCKET_CONF" ]                 || { warn "Config file missing";         errors=$((errors+1)); }
    [ -x "$SOCKROCKET_SCRIPTS/sockrocket.sh" ]      || { warn "sockrocket.sh not executable";       errors=$((errors+1)); }
    [ -x "$SOCKROCKET_SCRIPTS/iptables.sh" ] || { warn "iptables.sh not executable";  errors=$((errors+1)); }
    [ -f "$WWW_SOCKROCKET/sockrocket.asp" ]         || { warn "Web UI page missing";         errors=$((errors+1)); }
    [ -x "$SOCKROCKET_DIR/sockrocket.cgi" ]         || { warn "CGI script not executable";   errors=$((errors+1)); }
    [ "$errors" -eq 0 ] && ok "All files installed correctly" \
                         || warn "$errors validation warning(s) — check output above"

    echo ""
    ok "Sockrocket installed successfully!"
    echo ""
    echo "  Config file : $SOCKROCKET_CONF"
    echo "  Web UI      : http://router.asus.com/ext/Sockrocket/sockrocket.asp"
    echo ""
    echo "  Start  : $SOCKROCKET_SCRIPTS/sockrocket.sh start"
    echo "  Stop   : $SOCKROCKET_SCRIPTS/sockrocket.sh stop"
    echo "  Status : $SOCKROCKET_SCRIPTS/sockrocket.sh status"
    echo "  Update : sh $0 --update"
    echo ""
echo "Edit $SOCKROCKET_CONF to add your subscription or nodes, then start."
}

# ── Dispatch ──────────────────────────────────────────────────────────────────
if [ "$KOOLSHARE" = "1" ]; then
    ks_install
else
    merlin_install
fi
