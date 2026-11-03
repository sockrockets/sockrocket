#!/bin/sh
# Durable transaction worker; recovery runs before normal boot dispatch.
set -eu
umask 077
export SOCKROCKET_UPDATE_ACTIVE=1
ROOT=${SOCKROCKET_DIR:-/jffs/addons/sockrocket}
TX="$ROOT/.update-transaction"
SH="$ROOT/scripts/sockrocket.sh"
boot_id() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || echo unknown; }
claim_owner() {
    printf '%s\n' "$$" > "$TX/owner.new"
    mv "$TX/owner.new" "$TX/owner"
    boot_id > "$TX/boot"
}
NAMES='sockrocket-cli scripts webui res version uninstall.sh'
CONFIG_NAMES='config.yaml config.yaml.bak'
BACKUP_NAMES="$NAMES $CONFIG_NAMES"
health_check() {
    local port version attempt api_ok=0
    port=$(sed -n 's/^api_port:[[:space:]]*\([0-9]*\).*/\1/p' "$ROOT/config.yaml" | head -1)
    port=${port:-18188}
    version=$(cat "$ROOT/version") || return 1
    for attempt in 1 2 3 4 5; do
        if curl --max-time 5 -fsS -H "Authorization: Bearer $(cat "$ROOT/api-token")" "http://127.0.0.1:$port/api?action=version" > "$TX/api-response" &&
            grep -F "$version" "$TX/api-response" >/dev/null; then api_ok=1; break; fi
        sleep 1
    done
    [ "$api_ok" = 1 ] || return 1
    [ "$(cat "$TX/running")" = 1 ] || return 0
    kill -0 "$(cat "$ROOT/sockrocket.pid")" || return 1
    local probe timer rc dns_ok=0
    for attempt in 1 2 3 4 5 6; do
        nslookup example.com 127.0.0.1 > "$TX/dns-response" 2>&1 &
        probe=$!
        (trap - EXIT HUP INT TERM; sleep 10; kill -KILL "$probe" 2>/dev/null || true) &
        timer=$!
        rc=0
        wait "$probe" || rc=$?
        kill "$timer" 2>/dev/null || true
        wait "$timer" 2>/dev/null || true
        if [ "$rc" = 0 ]; then dns_ok=1; break; fi
        sleep 2
    done
    [ "$dns_ok" = 1 ] || return 1
    local socks_port
    socks_port=$(sed -n 's/^socks_port:[[:space:]]*\([0-9]*\).*/\1/p' "$ROOT/config.yaml" | head -1)
    socks_port=${socks_port:-1080}
    curl -fsS --connect-timeout 5 --max-time 12 --socks5-hostname "127.0.0.1:$socks_port" https://www.gstatic.com/generate_204 > /dev/null || return 1
    kill -0 "$(cat "$ROOT/sockrocket.pid")"
}
restore_files() {
    failed=0
    sh "$SH" stop >/dev/null 2>&1 || true
    sh "$SH" api-stop >/dev/null 2>&1 || true
    if [ -f "$TX/backup.tar.gz" ]; then
        # Free staged/new files before expanding the old installation.
        gzip -t "$TX/backup.tar.gz" || return 1
        rm -rf "$TX/new"
        for name in $NAMES; do rm -rf "$ROOT/$name" || return 1; done
        if [ -f "$TX/config-backed-up" ]; then
            for name in $CONFIG_NAMES; do rm -f "$ROOT/$name" || return 1; done
        fi
        tar -xzf "$TX/backup.tar.gz" -C "$ROOT" || return 1
    else
        # Compatibility with transactions created by the previous updater.
        for name in $NAMES; do
            if [ -e "$TX/old/$name" ]; then
                rm -rf "$ROOT/$name" || failed=1
                mv "$TX/old/$name" "$ROOT/$name" || failed=1
            fi
        done
    fi
    [ "$failed" = 0 ] || { echo 'rollback-failed: backup retained' > "$ROOT/update-status"; return 1; }
    if [ -f "$TX/external.tar.gz" ]; then
        gzip -t "$TX/external.tar.gz" || return 1
        tar -xzf "$TX/external.tar.gz" -C "$TX" || return 1
    fi
    if [ -f "$TX/external-list" ]; then
        while IFS= read -r path; do
            saved="$TX/external$path"
            if [ -e "$saved" ] || [ -L "$saved" ]; then
                rm -f "$path" || failed=1
                mv "$saved" "$path" || failed=1
            elif [ -f "$TX/absent$path" ]; then
                rm -f "$path" || failed=1
            fi
        done < "$TX/external-list"
    fi
    [ "$failed" = 0 ] || return 1
    # Restore all interfaces before starting a process that can trigger hooks.
    sh "$SH" api-restart || return 1
    if [ "$(cat "$TX/running")" = 1 ]; then sh "$SH" start || return 1; fi
    health_check || return 1
    echo "rolled-back: $(cat "$TX/stage" 2>/dev/null || echo interrupted)" > "$ROOT/update-status"
    rm -rf "$TX"
}
# Atomic directory creation serializes all recovery entry points. A second
# recovery never modifies files or takes ownership of the transaction.
restore() {
    local result=0
    mkdir "$ROOT/.update-restore-lock" 2>/dev/null || return 1
    claim_owner
    restore_files || result=$?
    rmdir "$ROOT/.update-restore-lock" || return 1
    [ "$result" = 0 ] || echo "rollback-failed: recovery health check failed; backup retained" > "$ROOT/update-status"
    return "$result"
}
if [ "${1:-}" = --recover ]; then
    [ -d "$TX" ] || exit 0
    if [ -f "$TX/owner" ] && [ -f "$TX/boot" ] && [ "$(cat "$TX/boot")" = "$(boot_id)" ]; then
        kill -0 "$(cat "$TX/owner")" 2>/dev/null && exit 0
    fi
    if [ -f "$TX/committed" ]; then rm -rf "$TX"; exit 0; fi
    if [ ! -f "$TX/prepared" ]; then rm -rf "$TX"; exit 0; fi
    restore
    exit $?
fi
PKG=$1
WORK=$2
# Count only files actually staged. Existing installed bytes are already
# deducted from df free space; they must not be reserved a second time.
check_space() {
    pkg_kb=$(du -sk "$PKG/bin/sockrocket-cli" "$PKG/scripts" "$PKG/webui" "$PKG/res" "$PKG/version" "$PKG/uninstall.sh" | awk '{s += $1} END {print s}')
    set --
    for name in $BACKUP_NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
    # Measure the real compressed backup without occupying flash first.
    tar -czf "$WORK/backup-probe.tar.gz" -C "$ROOT" "$@" || return 1
    gzip -t "$WORK/backup-probe.tar.gz" || return 1
    backup_kb=$(du -sk "$WORK/backup-probe.tar.gz" | awk '{print $1}')
    set --
    for name in $NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
    old_kb=$(cd "$ROOT" && du -sk "$@" | awk '{s += $1} END {print s+0}')
    growth_kb=$((pkg_kb - old_kb))
    [ "$growth_kb" -ge 0 ] || growth_kb=0
    required=$((growth_kb + backup_kb + 4096))
    available=$(df -Pk "$ROOT" | awk 'END {print $4}')
    case "$available" in ''|*[!0-9]*) echo 'Cannot determine free space' >&2; return 1;; esac
    if [ "$available" -lt "$required" ]; then
        echo "更新空间不足：$ROOT 需要 ${required} KiB，可用 ${available} KiB；现有安装未修改" >&2
        return 1
    fi
}
check_space || { echo "failed: insufficient space" > "$ROOT/update-status"; rm -rf "$WORK"; exit 1; }
[ "${3:-}" != --check-space ] || exit 0
# Compatibility is checked by the candidate itself, before any installed
# file changes or service interruption. The candidate does not rewrite config.
if ! "$PKG/bin/sockrocket-cli" --check-config "$ROOT/config.yaml" > "$WORK/config-check.log" 2>&1; then
    echo 'failed: configuration incompatible' > "$ROOT/update-status"
    echo '旧配置与新版不兼容；已取消升级，现有配置和程序保持不变' >&2
    exit 1
fi
config_hash=$(md5sum "$ROOT/config.yaml" | awk '{print $1}')
mkdir "$TX" || { rm -rf "$WORK"; exit 1; }
claim_owner
echo preparing > "$ROOT/update-status"
trap 'echo "failed: preparation" > "$ROOT/update-status"; rm -rf "$TX" "$WORK"' EXIT
"$PKG/bin/sockrocket-cli" --update-config-barrier "$ROOT/config.yaml"
trap 'echo "update failed; recovery required" > "$ROOT/update-status"; exit 1' HUP INT TERM
# Preparation never changes installed files. Failed preparation is safe to discard.
mkdir "$WORK/staged" "$TX/external" "$TX/absent"
# Recreate the backup under the transaction lock; the preflight copy is only an estimate.
set --
for name in $BACKUP_NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
tar -czf "$TX/backup.tar.gz" -C "$ROOT" "$@"
gzip -t "$TX/backup.tar.gz"
tar -tzf "$TX/backup.tar.gz" >/dev/null
touch "$TX/config-backed-up"
RUNNING=0
[ ! -f "$ROOT/sockrocket.pid" ] || ! kill -0 "$(cat "$ROOT/sockrocket.pid")" 2>/dev/null || RUNNING=1
printf '%s\n' "$RUNNING" > "$TX/running"
cp "$PKG/bin/sockrocket-cli" "$WORK/staged/"
cp -R "$PKG/scripts" "$PKG/webui" "$PKG/res" "$WORK/staged/"
# Match install_common_files: package layout differs from installed layout.
cp "$PKG/scripts/dnsmasq.conf" "$WORK/staged/scripts/dnsmasq.conf.template"
[ -s "$WORK/staged/scripts/dnsmasq.conf.template" ]
cp "$PKG/version" "$PKG/uninstall.sh" "$WORK/staged/"
chmod +x "$WORK/staged/sockrocket-cli" "$WORK/staged/scripts/"*.sh
# All installer-owned external files, including firmware hook files.
for path in /jffs/scripts/firewall-start /jffs/scripts/nat-start /jffs/scripts/services-start /jffs/scripts/service-event /jffs/scripts/wan-start /jffs/scripts/wan-event /jffs/scripts/dhcpc-event /jffs/scripts/dnsmasq.postconf /koolshare/webs/Module_sockrocket.asp /koolshare/scripts/sockrocket_config.sh /koolshare/scripts/sockrocket_api.sh /koolshare/scripts/sockrocket_status.sh /koolshare/scripts/uninstall_sockrocket.sh /koolshare/init.d/S98sockrocket.sh /koolshare/res/icon-sockrocket.png /www/ext/sockrocket/sockrocket.asp; do
    [ -d "$(dirname "$path")" ] || continue
    echo "$path" >> "$TX/external-list"
    mkdir -p "$TX/external$(dirname "$path")" "$TX/absent$(dirname "$path")"
    if [ -e "$path" ] || [ -L "$path" ]; then cp -a "$path" "$TX/external$path"; else touch "$TX/absent$path"; fi
done
tar -czf "$TX/external.tar.gz" -C "$TX" external
gzip -t "$TX/external.tar.gz"
rm -rf "$TX/external"
rollback() {
    trap - EXIT HUP INT TERM
    set +e
    claim_owner
    mkdir -p "$ROOT/update-diagnostics"
    for diagnostic in stage api-response dns-response; do
        [ ! -f "$TX/$diagnostic" ] || cp "$TX/$diagnostic" "$ROOT/update-diagnostics/"
    done
    restore
    rc=$?
    [ "$rc" = 0 ] || echo 'rollback-failed: recovery data retained' > "$ROOT/update-status"
    rm -rf "$WORK"
    exit 1
}
trap rollback EXIT HUP INT TERM
# Bootstrap is independent of scripts/, which can be interrupted mid-replacement.
cp "$WORK/worker.sh" "$ROOT/update-recover.sh.new"
mv "$ROOT/update-recover.sh.new" "$ROOT/update-recover.sh"
touch "$TX/prepared"
sync
for hook in /jffs/scripts/services-start /koolshare/init.d/S98sockrocket.sh; do
    [ -f "$hook" ] || continue
    sed '1a\
[ ! -f /jffs/addons/sockrocket/update-recover.sh ] || SOCKROCKET_DIR=/jffs/addons/sockrocket sh /jffs/addons/sockrocket/update-recover.sh --recover || exit 1
' "$hook" > "$TX/hook"
    cat "$TX/hook" > "$hook"
done
touch "$TX/prepared"
sync

# A concurrent settings save invalidates the compatibility check.
[ "$config_hash" = "$(md5sum "$ROOT/config.yaml" | awk '{print $1}')" ] || {
    trap - EXIT HUP INT TERM
    echo 'failed: configuration changed; retry' > "$ROOT/update-status"
    echo '配置在校验后发生变化；取消升级，请重试' >&2
    rm -rf "$TX" "$WORK"
    exit 1
}
echo stopping > "$TX/stage"
sh "$SH" stop
echo replacing > "$TX/stage"
for name in $NAMES; do
    rm -rf "$ROOT/$name"
    mv "$WORK/staged/$name" "$ROOT/$name"
done
# Reuse the package installer for firmware hooks and Software Center integration.
echo integration > "$TX/stage"
SOCKROCKET_INTEGRATION_ONLY=1 sh -e "$PKG/install.sh"
echo api-restart > "$TX/stage"
sh "$SH" api-restart
echo service-start > "$TX/stage"
[ "$RUNNING" = 0 ] || sh "$SH" start
echo health-check > "$TX/stage"
health_check

sync
touch "$TX/committed"
sync
trap - EXIT HUP INT TERM
echo 'success: plugin updated' > "$ROOT/update-status"
rm -rf "$TX" "$WORK"
