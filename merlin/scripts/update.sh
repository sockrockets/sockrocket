#!/bin/sh
# Durable transaction worker; recovery runs before normal boot dispatch.
set -eu
umask 077
export SOCKROCKET_UPDATE_ACTIVE=1
ROOT=${SOCKROCKET_DIR:-/jffs/addons/sockrocket}
TX="$ROOT/.update-transaction"
SH="$ROOT/scripts/sockrocket.sh"
NAMES='sockrocket-cli scripts webui res version uninstall.sh'
restore() {
    failed=0
    sh "$SH" stop >/dev/null 2>&1 || true
    if [ -f "$TX/backup.tar.gz" ]; then
        # Free staged/new files before expanding the old installation.
        gzip -t "$TX/backup.tar.gz" || return 1
        rm -rf "$TX/new"
        for name in $NAMES; do rm -rf "$ROOT/$name" || return 1; done
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
    sh "$SH" api-restart || return 1
    if [ "$(cat "$TX/running")" = 1 ]; then sh "$SH" start || return 1; fi
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
    echo "rolled-back: $(cat "$TX/stage" 2>/dev/null || echo interrupted)" > "$ROOT/update-status"
    rm -rf "$TX"
}
if [ "${1:-}" = --recover ]; then
    [ -d "$TX" ] || exit 0
    if [ -f "$TX/owner" ] && [ -f "$TX/boot" ] && [ "$(cat "$TX/boot")" = "$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)" ]; then
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
    for name in $NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
    # Measure the real compressed backup without occupying flash first.
    tar -czf "$WORK/backup-probe.tar.gz" -C "$ROOT" "$@" || return 1
    gzip -t "$WORK/backup-probe.tar.gz" || return 1
    backup_kb=$(du -sk "$WORK/backup-probe.tar.gz" | awk '{print $1}')
    old_kb=$(cd "$ROOT" && du -sk "$@" | awk '{s += $1} END {print s+0}')
    payload_kb=$pkg_kb
    [ "$old_kb" -le "$payload_kb" ] || payload_kb=$old_kb
    required=$((payload_kb + backup_kb + 4096))
    available=$(df -Pk "$ROOT" | awk 'END {print $4}')
    case "$available" in ''|*[!0-9]*) echo 'Cannot determine free space' >&2; return 1;; esac
    if [ "$available" -lt "$required" ]; then
        echo "更新空间不足：$ROOT 需要 ${required} KiB，可用 ${available} KiB；现有安装未修改" >&2
        return 1
    fi
}
check_space || { rm -rf "$WORK"; exit 1; }
[ "${3:-}" != --check-space ] || exit 0
mkdir "$TX" || { rm -rf "$WORK"; exit 1; }
echo "$$" > "$TX/owner"
cat /proc/sys/kernel/random/boot_id > "$TX/boot"
trap 'echo "update failed; recovery required" > "$ROOT/update-status"; exit 1' HUP INT TERM
# Preparation never changes installed files. Failed preparation is safe to discard.
mkdir "$TX/new" "$TX/external" "$TX/absent"
# Recreate the backup under the transaction lock; the preflight copy is only an estimate.
set --
for name in $NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
tar -czf "$TX/backup.tar.gz" -C "$ROOT" "$@"
gzip -t "$TX/backup.tar.gz"
tar -tzf "$TX/backup.tar.gz" >/dev/null
RUNNING=0
[ ! -f "$ROOT/sockrocket.pid" ] || ! kill -0 "$(cat "$ROOT/sockrocket.pid")" 2>/dev/null || RUNNING=1
printf '%s\n' "$RUNNING" > "$TX/running"
cp "$PKG/bin/sockrocket-cli" "$TX/new/"
cp -R "$PKG/scripts" "$PKG/webui" "$PKG/res" "$TX/new/"
cp "$PKG/version" "$PKG/uninstall.sh" "$TX/new/"
chmod +x "$TX/new/sockrocket-cli" "$TX/new/scripts/"*.sh
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
rollback() {
    trap - EXIT HUP INT TERM
    set +e
    restore
    rc=$?
    [ "$rc" = 0 ] || echo 'rollback-failed: recovery data retained' > "$ROOT/update-status"
    rm -rf "$WORK"
    exit 1
}
trap rollback EXIT HUP INT TERM
echo stopping > "$TX/stage"
sh "$SH" stop
echo replacing > "$TX/stage"
for name in $NAMES; do
    rm -rf "$ROOT/$name"
    mv "$TX/new/$name" "$ROOT/$name"
done
# Reuse the package installer for firmware hooks and Software Center integration.
echo integration > "$TX/stage"
SOCKROCKET_INTEGRATION_ONLY=1 sh -e "$PKG/install.sh"
echo api-restart > "$TX/stage"
sh "$SH" api-restart
echo service-start > "$TX/stage"
[ "$RUNNING" = 0 ] || sh "$SH" start
echo api-health > "$TX/stage"
# Probe the actual management listener, not just a surviving PID.
port=$(sed -n 's/^api_port:[[:space:]]*\([0-9]*\).*/\1/p' "$ROOT/config.yaml" | head -1)
port=${port:-18188}
curl --max-time 10 -fsS -H "Authorization: Bearer $(cat "$ROOT/api-token")" "http://127.0.0.1:$port/api?action=version" > "$TX/api-response"
version=$(cat "$ROOT/version")
grep -F "$version" "$TX/api-response" >/dev/null
if [ "$RUNNING" = 1 ]; then
    sleep 5
    kill -0 "$(cat "$ROOT/sockrocket.pid")"
    # Query the router DNS path used by LAN clients.
    echo dns-health > "$TX/stage"
    nslookup example.com 127.0.0.1 > "$TX/dns-response" 2>&1
fi
sync
touch "$TX/committed"
sync
trap - EXIT HUP INT TERM
echo 'success: plugin updated' > "$ROOT/update-status"
rm -rf "$TX" "$WORK"
