#!/bin/sh
# Authenticated firmware RPC adapter. Per-request dbus keys isolate tabs;
# results are private, atomically published, acknowledged and short-lived.
. /koolshare/scripts/base.sh
umask 077
SOCKROCKET_BIN="${SOCKROCKET_BIN:-/jffs/addons/sockrocket/sockrocket-cli}"
UPLOAD_DIR="${SOCKROCKET_RPC_UPLOAD_DIR:-/tmp/upload}"
RUNTIME_DIR="${SOCKROCKET_RPC_RUNTIME_DIR:-/tmp/sockrocket-rpc}"
ACTION="${2:-$1}"
SEQ="$3"
ENCODING=raw
respond() {
    curl --max-time 3 -fsS -X POST --data "$ID" "http://127.0.0.1:3030/_resp/$ID" >/dev/null 2>&1 || true
}
# New clients pass a cryptographic nonce; retain compatibility with old pages.
if [ -n "$SEQ" ]; then
    case "$SEQ" in *[!0-9a-f]*|'') respond; exit 1 ;; esac
    [ "${#SEQ}" -eq 32 ] || { respond; exit 1; }
    # Some Softcenter versions shell-escape every quote/backslash in fields.
    # Base64 carries UTF-8 JSON without those characters; never eval/unescape it.
    ARGS=$(dbus get "sockrocket_rpc_b64_$SEQ" 2>/dev/null)
    if [ -n "$ARGS" ]; then
        ENCODING=base64
    else
        ARGS=$(dbus get "sockrocket_rpc_body_$SEQ" 2>/dev/null)
    fi
    dbus remove "sockrocket_rpc_b64_$SEQ" >/dev/null 2>&1 || true
    dbus remove "sockrocket_rpc_body_$SEQ" >/dev/null 2>&1 || true
else
    SEQ=$(dbus get sockrocket_rpc_seq 2>/dev/null)
    case "$SEQ" in ''|*[!0-9]*) respond; exit 1 ;; esac
    [ "${#SEQ}" -le 24 ] || { respond; exit 1; }
    ARGS=$(dbus get sockrocket_rpc_args 2>/dev/null)
    dbus remove sockrocket_rpc_args >/dev/null 2>&1 || true
fi
case "$ACTION" in ''|*[!a-z_]* ) respond; exit 1 ;; esac
mkdir -p "$UPLOAD_DIR" "$RUNTIME_DIR" || exit 1
RESULT="$UPLOAD_DIR/sockrocket_rpc_${SEQ}.json"
if [ "$ACTION" = rpc_ack ]; then
    rm -f "$RESULT"
    respond
    exit 0
fi
# Compatibility pages may not acknowledge; bound leftover data to minutes.
find "$UPLOAD_DIR" -maxdepth 1 -name 'sockrocket_rpc_*.json' -mmin +2 -exec rm -f {} + 2>/dev/null || true
RAW="$RUNTIME_DIR/response-$SEQ.$$"
TEMP="$RUNTIME_DIR/result-$SEQ.$$"
SLOT=""
[ -n "$ARGS" ] || ARGS="{}"
cleanup() { rm -f "$RAW" "$TEMP"; [ -z "$SLOT" ] || rm -rf "$SLOT"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# The firmware starts a fresh CGI process per job: cap these independently
# of the long-lived HTTP API. Reserve Stop outside the normal work slots.
if [ "$ACTION" = stop ]; then slots="stop"; else slots="1 2 3 4"; fi
for n in $slots; do
    slot="$RUNTIME_DIR/slot-$n"
    if [ -f "$slot/pid" ]; then
        pid=$(cat "$slot/pid")
        case "$pid" in ''|*[!0-9]*) ;; *) kill -0 "$pid" 2>/dev/null || rm -rf "$slot" ;; esac
    fi
    if mkdir "$slot" 2>/dev/null; then SLOT="$slot"; echo $$ > "$slot/pid"; break; fi
done
# The dbus body is a new request; inherited HTTP CONTENT_LENGTH must not
# truncate its JSON when forwarding to the CGI backend.
if [ -z "$SLOT" ]; then
    printf '{"error":"Router busy; retry shortly","error_code":"busy"}' > "$TEMP"
elif [ ! -x "$SOCKROCKET_BIN" ]; then
    printf '{"ok":false,"error":"Backend executable unavailable","error_code":"backend_unavailable","stage":"invoke_backend","cause":"NotFound","hint":"通过软件中心重新安装适配机型的完整插件包。"}' > "$TEMP"
elif printf '%s' "$ARGS" | SOCKROCKET_LOCAL_RPC=1 SOCKROCKET_RPC_ENCODING="$ENCODING" REQUEST_METHOD=POST CONTENT_LENGTH= QUERY_STRING="action=$ACTION" "$SOCKROCKET_BIN" cgi > "$RAW" 2>/dev/null; then
    sed '1,3d' "$RAW" > "$TEMP"
    [ -s "$TEMP" ] || printf '{"ok":false,"error":"Empty backend response","error_code":"backend_empty_response","stage":"invoke_backend","hint":"通过软件中心重新安装完整插件包，并刷新页面。"}' > "$TEMP"
else
    backend_status=$?
    # Raw stderr can contain subscription credentials. Only expose exit status.
    printf '{"ok":false,"error":"Backend process failed","error_code":"backend_failed","stage":"invoke_backend","exit_code":%s,"hint":"记录退出码并检查系统日志；通过软件中心重新安装完整插件包，勿单独替换二进制。"}' "$backend_status" > "$TEMP"
fi
mv -f "$TEMP" "$RESULT" || exit 1
chmod 600 "$RESULT"
respond
