#!/bin/sh
# Sockrocket API bridge for the koolcenter software center (rogsoft platform).
#
# The platform dispatches POST /_api/ {"method": "sockrocket_api.sh", "params":
# [<action>], "fields": {...}} to this script. It runs the Rust CGI built
# into sockrocket-cli synchronously (sub-second for all read actions) and writes
# the JSON body to /tmp/upload/sockrocket_rpc_<seq>.json, which the page then
# polls via GET /_temp/sockrocket_rpc_<seq>.json.
#
# Input via dbus keys (set by the page's "fields"):
#   sockrocket_rpc_seq  — unique request id, names the result file
#   sockrocket_rpc_args — JSON POST body for the action (optional)

source /koolshare/scripts/base.sh

ACTION="$1"
SOCKROCKET_BIN="/jffs/addons/sockrocket/sockrocket-cli"
SEQ=$(dbus get sockrocket_rpc_seq 2>/dev/null)
# Only digits — dbus value is interpolated into a filesystem path.
case "$SEQ" in
    ''|*[!0-9]*) SEQ="$$" ;;
esac
ARGS=$(dbus get sockrocket_rpc_args 2>/dev/null)
RESULT="/tmp/upload/sockrocket_rpc_${SEQ}.json"

mkdir -p /tmp/upload 2>/dev/null

# Bound tmpfs growth: drop RPC result files older than ~1 hour. `find -mmin`
# is available on Merlin busybox; ignore failures on exotic builds.
find /tmp/upload -maxdepth 1 -name 'sockrocket_rpc_*.json' -mmin +60 -exec rm -f {} + 2>/dev/null || true

if [ ! -x "$SOCKROCKET_BIN" ]; then
    printf '{"ok":false,"msg":"sockrocket-cli not installed"}' > "$RESULT"
    http_response "Sockrocket ${ACTION}: binary missing" >/dev/null 2>&1
    exit 1
fi

# The Rust CGI prints 3 header lines (Content-Type, Cache-Control, blank)
# before the JSON body — strip them. POST body goes via stdin with
# REQUEST_METHOD set and no CONTENT_LENGTH (CGI reads stdin to EOF).
if [ -n "$ARGS" ]; then
    printf '%s' "$ARGS" | REQUEST_METHOD=POST QUERY_STRING="action=$ACTION" "$SOCKROCKET_BIN" cgi 2>/dev/null | sed '1,3d' > "$RESULT"
else
    QUERY_STRING="action=$ACTION" "$SOCKROCKET_BIN" cgi 2>/dev/null | sed '1,3d' > "$RESULT"
fi

# Tell the platform job system we are done.
http_response "Sockrocket ${ACTION} ok" >/dev/null 2>&1
exit 0
