#!/bin/sh
# Software Center runtime probe. API availability must not imply proxy enabled.
umask 077
CONTROL=/jffs/addons/sockrocket/scripts/sockrocket.sh
result='Sockrocket stopped'
if [ -x "$CONTROL" ]; then
    result=$("$CONTROL" software-center-status 2>/dev/null) || result='Sockrocket stopped'
fi
case "${1:-}" in
    ''|*[!0-9]*) printf '%s\n' "$result" ;;
    *) curl --max-time 3 -fsS -X POST --data "$result" \
        "http://127.0.0.1:3030/_resp/$1" >/dev/null 2>&1 ;;
esac
