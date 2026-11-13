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
FILES='scripts webui res version uninstall.sh'
NAMES="sockrocket-cli $FILES"
CONFIG_NAMES='config.yaml config.yaml.bak'
BACKUP_NAMES="$FILES $CONFIG_NAMES"
# An installed control script may trust a recycled pid, especially for its
# guard/supervisor. Verify the actual argv before letting that script stop it.
pid_matches() {
    local pid="$1" role="$2" args
    case "$pid" in ''|*[!0-9]*|0*|1) return 1 ;; esac
    kill -0 "$pid" 2>/dev/null || return 1
    args="${SOCKROCKET_PROC_BASE:-/proc}/$pid/cmdline"
    [ -r "$args" ] || return 1
    tr '\000' '\n' < "$args" | awk -v root="$ROOT" -v role="$role" '
        NR == 1 { exe=$0 }
        NR == 2 { arg=$0 }
        END {
            cli=(exe == root "/sockrocket-cli")
            control=root "/scripts/sockrocket.sh"
            if (role == "daemon") exit !(cli && arg == root "/config.yaml")
            if (role == "api") exit !(cli && arg == "api")
            exit !(exe == control || (exe ~ /(^|\/)(a?sh)$/ && arg == control))
        }'
}
clear_stale_pids() {
    local file role pid
    for file in sockrocket.pid sockrocket-api.pid sockrocket-supervisor.pid sockrocket-guard.pid; do
        [ -f "$ROOT/$file" ] || continue
        pid=$(cat "$ROOT/$file") || return 1
        case "$file" in sockrocket.pid) role=daemon ;; sockrocket-api.pid) role=api ;; *) role=control ;; esac
        pid_matches "$pid" "$role" && continue
        [ "$(cat "$ROOT/$file" 2>/dev/null)" != "$pid" ] || rm -f "$ROOT/$file" || return 1
    done
    return 0
}
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
    pid_matches "$(cat "$ROOT/sockrocket.pid")" daemon || return 1
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
    [ "$dns_ok" = 1 ] || return 2
    local socks_port
    socks_port=$(sed -n 's/^socks_port:[[:space:]]*\([0-9]*\).*/\1/p' "$ROOT/config.yaml" | head -1)
    socks_port=${socks_port:-1080}
    curl -fsS --connect-timeout 5 --max-time 12 --socks5-hostname "127.0.0.1:$socks_port" https://www.gstatic.com/generate_204 > /dev/null || return 2
    pid_matches "$(cat "$ROOT/sockrocket.pid")" daemon
}
restore_files() {
    local external_root="$TX"
    failed=0
    clear_stale_pids || return 1
    sh "$SH" stop >/dev/null 2>&1 || true
    sh "$SH" api-stop >/dev/null 2>&1 || true
    if [ -f "$TX/backup.tar.gz" ]; then
        # Free staged/new files before expanding the old installation.
        gzip -t "$TX/backup.tar.gz" || return 1
        rm -rf "$TX/new" || return 1
        local archived="$NAMES"
        if [ -f "$TX/cli-kept" ]; then
            # The old executable is always at one of these two paths. Renaming
            # it back is atomic. A retry after the rename keeps that same inode.
            if [ -f "$TX/previous-cli" ]; then
                mv -f "$TX/previous-cli" "$ROOT/sockrocket-cli" || return 1
            fi
            [ -f "$ROOT/sockrocket-cli" ] || return 1
            archived="$FILES"
        fi
        for name in $archived; do rm -rf "$ROOT/$name" || return 1; done
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
        # Legacy CGI entries can each be a full CLI. Expand onto the same
        # temporary filesystem used to prepare the snapshot, not onto JFFS.
        external_root=$1
        tar -xzf "$TX/external.tar.gz" -C "$external_root" || return 1
    fi
    if [ -f "$TX/external-list" ]; then
        while IFS= read -r path; do
            saved="$external_root/external$path"
            if [ -e "$saved" ] || [ -L "$saved" ]; then
                mkdir -p "${path%/*}" && rm -f "$path" && mv "$saved" "$path" || failed=1
            elif [ -f "$TX/absent$path" ]; then
                rm -f "$path" || failed=1
            fi
        done < "$TX/external-list"
    fi
    [ "$failed" = 0 ] || return 1
    # Restore all interfaces before starting a process that can trigger hooks.
    sh "$SH" api-restart || return 1
    if [ "$(cat "$TX/running")" = 1 ]; then sh "$SH" start || return 1; fi
    local health=0
    health_check || health=$?
    case "$health" in
        0) echo "rolled-back: $(cat "$TX/stage" 2>/dev/null || echo interrupted)" > "$ROOT/update-status" ;;
        2)
            # A broken DNS/upstream can affect both versions. Once the restored
            # API and daemon are up, release the transaction so it can be fixed.
            pid_matches "$(cat "$ROOT/sockrocket.pid")" daemon || return 1
            echo "failed: 已恢复旧版，可以修改设置。网络检查失败，请检查 DNS 和代理节点。" > "$ROOT/update-status" ;;
        *) return 1 ;;
    esac
    rm -rf "$TX"
}
# Atomic directory creation serializes all recovery entry points. A second
# recovery never modifies files or takes ownership of the transaction.
restore() {
    local result=0
    # Keep this lock off JFFS. A power loss during rollback must allow
    # the next boot to claim the retained transaction.
    local runtime="${SOCKROCKET_RUNTIME_DIR:-/tmp/sockrocket-run}"
    mkdir -p "$runtime" || return 1
    mkdir "$runtime/.update-restore-lock" 2>/dev/null || return 1
    claim_owner
    restore_files "$runtime/.update-restore-lock" || result=$?
    rm -rf "$runtime/.update-restore-lock/external" || return 1
    rmdir "$runtime/.update-restore-lock" || return 1
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
mode=${3:-install}
# Package data is in /tmp. Keep the installed CLI inode on flash through
# commit or rollback, including when a process still has it mapped.
size_kb() {
    local sizes
    sizes=$(du -sk "$@") || return 1
    printf '%s\n' "$sizes" | awk '{s += $1} END {print s+0}'
}
# Keep one budget per filesystem. The firmware may mount the Software Center
# separately, and standard Merlin can publish its page on either of two mounts.
record_space() {
    local kind="$1" kb="$2" parent="${3%/*}" dev
    while [ ! -d "$parent" ]; do
        [ ! -e "$parent" ] && [ ! -L "$parent" ] || return 1
        parent=${parent%/*}
        [ -n "$parent" ] || parent=/
    done
    # Multiple independent tmpfs mounts may all report "tmpfs" in df.
    dev=$(stat -L -c %d "$parent" 2>/dev/null) || dev=$(stat -L -f %d "$parent" 2>/dev/null) ||
        dev=$(df -Pk "$parent" | awk 'END {if ($4 ~ /^[0-9]+$/) print $1 ":" $6}')
    [ -n "$dev" ] || { echo "无法读取 $parent 的可用空间" >&2; return 1; }
    space_rows="$space_rows$(printf '%s\t%s\t%s\t%s' "$dev" "$kind" "$kb" "$parent")
"
}
add_integration_file() {
    local bytes
    [ -f "$1" ] || return 0
    bytes=$(wc -c < "$1") || return 1
    record_space add "$(((bytes + 4095) / 4096 * 4))" "$2"
}
require_space() {
    local root_kb="$1" phase="$2" rows path base page available need message="" page_error=""
    local page_ok=0
    rows=$(printf '%s' "$space_rows" | awk -F '\t' -v root_kb="$root_kb" '
        { dev=$1; if (!(dev in path)) path[dev]=$4 }
        $2=="root" { sum[dev]+=root_kb; path[dev]=$4 }
        $2=="add"  { sum[dev]+=$3 }
        $2=="hook" && $3>hook[dev] { hook[dev]=$3 }
        $2=="page" { page[dev]=$3 }
        END { for (d in path) printf "%s\t%d\t%d\n",path[d],sum[d]+hook[d],page[d] }
    ') || return 1
    while IFS="$(printf '\t')" read -r path base page; do
        available=$(df -Pk "$path" | awk 'END {print $4}')
        case "$available" in ''|*[!0-9]*) message="无法读取 $path 的可用空间"; break ;; esac
        if [ "$available" -lt "$base" ]; then
            message="$phase 空间不足：$path 需要 ${base} KiB，可用 ${available} KiB，还差 $((base - available)) KiB"
            break
        fi
        [ "$page" -gt 0 ] || continue
        need=$((base + page))
        if [ "$available" -ge "$need" ]; then page_ok=1; else
            page_error="$phase 空间不足：$path 需要 ${need} KiB，可用 ${available} KiB，还差 $((need - available)) KiB"
        fi
    done <<EOF
$rows
EOF
    if [ -z "$message" ] && [ "$page_ok" = 0 ] && [ ! -f /koolshare/scripts/base.sh ] && [ -s "$PKG/webui/sockrocket.asp" ]; then
        message=${page_error:-没有可用的 Merlin 网页安装目录}
    fi
    [ -n "$message" ] || return 0
    if [ "$mode" != --check-space ]; then
        printf 'failed: %s\n' "$message" > "$ROOT/update-status"
    fi
    printf '%s\n' "$message" >&2
    return 1
}
# Build the backup archives in tmpfs; only the compressed archives, external
# file list and absence markers need to coexist with the new install on flash.
snapshot="$WORK/backup"
prepare_snapshot() {
    [ -f "$ROOT/sockrocket-cli" ] && [ ! -L "$ROOT/sockrocket-cli" ] || {
        echo 'Installed CLI must be a regular file' >&2
        return 1
    }
    rm -rf "$snapshot"
    mkdir -p "$snapshot/external" "$snapshot/absent"
    set --
    for name in $BACKUP_NAMES; do [ ! -e "$ROOT/$name" ] || set -- "$@" "$name"; done
    # Recovery uses this marker to distinguish the older all-in-one archives.
    touch "$snapshot/cli-kept"
    tar -czf "$snapshot/backup.tar.gz" -C "$ROOT" "$@"
    gzip -t "$snapshot/backup.tar.gz"
    space_rows=""
    record_space root 0 "$ROOT/sockrocket-cli"
    # All installer-owned external files, including firmware hook files.
    for path in "$ROOT/sockrocket.cgi" /www/cgi-bin/sockrocket.cgi /jffs/scripts/firewall-start /jffs/scripts/nat-start /jffs/scripts/services-start /jffs/scripts/service-event /jffs/scripts/wan-start /jffs/scripts/wan-event /jffs/scripts/dhcpc-event /jffs/scripts/dnsmasq.postconf /koolshare/webs/Module_sockrocket.asp /jffs/.koolshare/webs/Module_sockrocket.asp /koolshare/scripts/sockrocket_config.sh /koolshare/scripts/sockrocket_api.sh /koolshare/scripts/sockrocket_status.sh /koolshare/scripts/uninstall_sockrocket.sh /koolshare/init.d/S98sockrocket.sh /koolshare/res/icon-sockrocket.png /www/ext/sockrocket/sockrocket.asp /tmp/var/wwwext/sockrocket/sockrocket.asp; do
        case "$path" in
            /www/cgi-bin/sockrocket.cgi|/www/ext/sockrocket/sockrocket.asp|/tmp/var/wwwext/sockrocket/sockrocket.asp)
                # These entries are published with rename. Firmware may expose
                # a read-only primary tree or create its web directories at boot.
                parent=${path%/*}
                while [ ! -d "$parent" ] && [ -n "$parent" ]; do
                    if [ -e "$parent" ] || [ -L "$parent" ]; then continue 2; fi
                    parent=${parent%/*}
                done
                [ -n "$parent" ] || continue
                probe="$parent/.sockrocket-update.$$"
                mkdir "$probe" 2>/dev/null || continue
                rmdir "$probe" || return 1
                if [ "${path##*/}" = sockrocket.asp ] && [ -s "$PKG/webui/sockrocket.asp" ]; then
                    bytes=$(wc -c < "$PKG/webui/sockrocket.asp")
                    record_space page "$(((bytes + 4095) / 4096 * 4 + 4))" "$path"
                fi
                ;;
            /jffs/.koolshare/webs/Module_sockrocket.asp)
                [ -d "${path%/*}" ] && ! [ /koolshare/webs -ef /jffs/.koolshare/webs ] || continue ;;
            *) [ -d "${path%/*}" ] || continue ;;
        esac
        case "$path" in
            /jffs/scripts/*|/koolshare/init.d/S98sockrocket.sh)
                if [ -f "$path" ]; then
                    kb=$(wc -c < "$path")
                    kb=$(((kb + 1023) / 1024))
                    record_space hook "$kb" "$path"
                fi
                ;;
        esac
        echo "$path" >> "$snapshot/external-list"
        mkdir -p "$snapshot/external$(dirname "$path")" "$snapshot/absent$(dirname "$path")"
        if [ -e "$path" ] || [ -L "$path" ]; then cp -a "$path" "$snapshot/external$path"; else touch "$snapshot/absent$path"; fi
    done
    tar -czf "$snapshot/external.tar.gz" -C "$snapshot" external
    gzip -t "$snapshot/external.tar.gz"
    rm -rf "$snapshot/external"
    if [ -f /koolshare/scripts/base.sh ]; then
        add_integration_file "$PKG/webui/sockrocket.asp" /koolshare/webs/Module_sockrocket.asp
        if [ -d /jffs/.koolshare/webs ] && ! [ /koolshare/webs -ef /jffs/.koolshare/webs ]; then
            add_integration_file "$PKG/webui/sockrocket.asp" /jffs/.koolshare/webs/Module_sockrocket.asp
        fi
        icon="$PKG/res/icon-sockrocket.png"
        [ -f "$icon" ] || icon="$PKG/webui/icon-sockrocket.png"
        add_integration_file "$icon" /koolshare/res/icon-sockrocket.png
        record_space add 8 /koolshare/scripts/sockrocket_config.sh
        record_space add 4 /koolshare/init.d/S98sockrocket.sh
        for helper in sockrocket_api.sh sockrocket_status.sh; do
            add_integration_file "$PKG/scripts/$helper" "/koolshare/scripts/$helper"
        done
        add_integration_file "$PKG/uninstall.sh" /koolshare/scripts/uninstall_sockrocket.sh
    fi
    # Charge the new CLI at logical size even on a compressed source filesystem.
    # Keep 256 KiB for small generated hooks, status files and allocation rounding.
    cli_bytes=$(wc -c < "$PKG/bin/sockrocket-cli")
    assets_kb=$(size_kb "$PKG/scripts" "$PKG/webui" "$PKG/res" "$PKG/version" "$PKG/uninstall.sh")
    worker_kb=$(size_kb "$0")
    install_kb=$(((cli_bytes + 1023) / 1024 + assets_kb + worker_kb + 256))
}
if [ "$mode" = --check-space ]; then
    trap 'rm -rf "$snapshot"' EXIT
    prepare_snapshot
    backup_kb=$(size_kb "$snapshot")
    require_space "$((backup_kb + install_kb))" "升级前"
    exit 0
fi
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
trap 'rm -rf "$TX" "$WORK"; echo "failed: preparation" > "$ROOT/update-status"' EXIT
"$PKG/bin/sockrocket-cli" --update-config-barrier "$ROOT/config.yaml"
trap 'echo "update failed; recovery required" > "$ROOT/update-status"; exit 1' HUP INT TERM
# Preparation never changes installed files. Failed preparation is safe to discard.
prepare_snapshot
backup_kb=$(size_kb "$snapshot")
if ! require_space "$((backup_kb + install_kb))" "升级前"; then
    trap - EXIT HUP INT TERM
    rm -rf "$TX" "$WORK"
    exit 1
fi
cp -R "$snapshot/." "$TX/"
gzip -t "$TX/backup.tar.gz"
gzip -t "$TX/external.tar.gz"
rm -rf "$snapshot"
touch "$TX/config-backed-up"
# Boot and watchdog both use this persistent marker for the user's intent.
# A temporarily crashed daemon must still be restarted after the transaction.
RUNNING=1
[ ! -f "$ROOT/stopped" ] || RUNNING=0
printf '%s\n' "$RUNNING" > "$TX/running"
# Backups now occupy flash. Re-read df before arming rollback or stopping services.
if ! require_space "$install_kb" "备份后"; then
    trap - EXIT HUP INT TERM
    rm -rf "$TX" "$WORK"
    exit 1
fi
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
clear_stale_pids
sh "$SH" stop
sh "$SH" api-stop
# Newly opened files or concurrent writers may have changed free space while
# stopping. Rollback is already durable and can restart the previous service.
require_space "$install_kb" "停止服务后"
echo replacing > "$TX/stage"
mv "$ROOT/sockrocket-cli" "$TX/previous-cli"
# The candidate must never become durable before the old inode's new name.
sync
cp "$PKG/bin/sockrocket-cli" "$ROOT/sockrocket-cli"
for name in $FILES; do
    rm -rf "$ROOT/$name"
    cp -R "$PKG/$name" "$ROOT/$name"
done
# Match the installed layout without duplicating the extracted payload in /tmp.
mv "$ROOT/scripts/dnsmasq.conf" "$ROOT/scripts/dnsmasq.conf.template"
test -s "$ROOT/scripts/dnsmasq.conf.template"
chmod +x "$ROOT/sockrocket-cli" "$ROOT/scripts/"*.sh
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
