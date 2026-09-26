#!/bin/sh
# Test harness for merlin/scripts/sockrocket.sh stop / watchdog semantics.
#
# Runs the real script against a sandbox SOCKROCKET_DIR with a fake `sockrocket-cli`,
# `iptables.sh` and stubbed router commands (logger/cru/service/nvram/
# netstat/ip), so the stop path can be exercised on a dev machine with no
# router attached.
#
# Usage: sh tests/integration/sockrocket_sh_test.sh
set -u

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SOCKROCKET_SH="$REPO_ROOT/merlin/scripts/sockrocket.sh"

PASS=0
FAIL=0

assert() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        PASS=$((PASS + 1))
        printf '  ok   - %s\n' "$desc"
    else
        FAIL=$((FAIL + 1))
        printf '  FAIL - %s\n         expected: [%s]\n         actual:   [%s]\n' \
            "$desc" "$expected" "$actual"
    fi
}

# `is_running` (both in sockrocket.sh and in sockrocket-cli) matches /proc/<pid>/comm
# against *sockrocket-cli*. A copy of `sleep` renamed to sockrocket-cli gives us a real
# process the script accepts, on any platform �?but `sleep` rejects the
# `config.yaml --tun` argv that do_start hands the daemon, so it would die
# instantly and do_start would take its "failed to start" path. The
# trampoline below execs the renamed copy with a fixed numeric argument, so
# the process survives any argv while its comm still contains "sockrocket-cli"
# (exec keeps the PID do_start recorded in sockrocket.pid).
#
# It also has to fabricate that comm file: MSYS/Windows has no
# /proc/<pid>/comm at all (only cmdline/exe/fd), so on a dev box there
# is_running() would always be false and every is_running-gated branch of
# do_start / do_watchdog would be skipped silently �?the suite would report
# "pass" for tests whose subject never ran. Linux keeps the real file and
# exercises the genuine path.
make_fake_bin() {
    src=$(command -v sleep)
    cp "$src" "$SOCKROCKET_DIR/sockrocket-cli-bin"
    chmod +x "$SOCKROCKET_DIR/sockrocket-cli-bin"
    cat > "$SOCKROCKET_DIR/sockrocket-cli" <<SH
#!/bin/sh
mkdir -p "$PROC_FAKE/\$\$" 2>/dev/null
echo "sockrocket-cli" > "$PROC_FAKE/\$\$/comm" 2>/dev/null
exec "$SOCKROCKET_DIR/sockrocket-cli-bin" 300
SH
    chmod +x "$SOCKROCKET_DIR/sockrocket-cli"
}

# ── Sandbox ───────────────────────────────────────────────────────────────────
# sockrocket.sh hardcodes SOCKROCKET_DIR=/jffs/addons/sockrocket (it runs on a router), so the
# suite copies the script with that path rewritten to the sandbox �?it still
# executes the real logic, just pointed somewhere writable.
setup() {
    SANDBOX=$(mktemp -d)
    SOCKROCKET_DIR="$SANDBOX/Sockrocket"
    DNS_DIR="$SANDBOX/dnsmasq.d"
    PROC_FAKE="$SANDBOX/proc"
    mkdir -p "$SOCKROCKET_DIR/scripts" "$SANDBOX/stub" "$DNS_DIR" "$PROC_FAKE"

    sed "s#^SOCKROCKET_DIR=\"/jffs/addons/sockrocket\"#SOCKROCKET_DIR=\"$SOCKROCKET_DIR\"#" "$SOCKROCKET_SH" \
        > "$SANDBOX/sockrocket.sh"

    cat > "$SOCKROCKET_DIR/config.yaml" <<'YAML'
mode: "tun"
transparent_proxy: true
dns_hijack: true
socks_port: 1080
http_port: 1087
dns_port: 5300
api_port: 18188
dns_direct_server: "114.114.114.114"
active_node: 0
subscriptions: []
rules: []
dns_direct_domains:
  - "baidu.com"
nodes:
  - name: "Node A"
    server: "a.example.com"
    port: 443
YAML

    make_fake_bin

    # iptables.sh stand-in: records every call and models the mangle chain
    # as a marker file, so the test can see whether the rules were torn down.
    cat > "$SOCKROCKET_DIR/scripts/iptables.sh" <<SH
#!/bin/sh
echo "iptables.sh \$1" >> "$SANDBOX/iptables.calls"
case "\$1" in
    stop)  rm -f "$SANDBOX/mangle_chain" ;;
    start) touch "$SANDBOX/mangle_chain" ;;
esac
exit 0
SH
    chmod +x "$SOCKROCKET_DIR/scripts/iptables.sh"

    # dnsmasq template + the live hijack file the script writes/removes.
    printf '# stub template __DNS_PORT__ __ROUTER_IP__\n' \
        > "$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"
    touch "$DNS_DIR/sockrocket.conf"

    : > "$SANDBOX/iptables.calls"
    : > "$SANDBOX/cru.calls"
    : > "$SANDBOX/logger.calls"
    : > "$SANDBOX/modprobe.calls"
    : > "$SANDBOX/insmod.calls"
    touch "$SANDBOX/tun_dev"
    # The firmware-side dnsmasq servers-file that pin_dns_upstream overwrites
    # and unpin_dns_upstream must restore. ISP DNS placeholder content, plus
    # the system resolver file the no-backup fallback rebuilds from.
    echo "server=192.168.1.1" > "$SANDBOX/resolv.dnsmasq"
    printf 'nameserver 127.0.0.1\nnameserver 192.168.1.1\n' > "$SANDBOX/resolv.conf"

    # The logger stub records what the script logs. This is the reliable way
    # to observe control flow: sockrocket.log itself can't be used, because do_start's
    # failure path re-logs the tail of the log (duplicating its own lines).
    # Emits "<prio> <message>" so assertions can anchor to the exact call.
    cat > "$SANDBOX/stub/logger" <<'SH'
#!/bin/sh
prio=""; msg=""
while [ $# -gt 0 ]; do
    case "$1" in
        -t) shift 2 ;;
        -p) prio="$2"; shift 2 ;;
        *)  msg="$msg $1"; shift ;;
    esac
done
echo "$prio ${msg# }" >> "$SANDBOX/logger.calls"
exit 0
SH
    cat > "$SANDBOX/stub/nvram" <<'SH'
#!/bin/sh
[ "$1" = "get" ] && [ "$2" = "lan_ipaddr" ] && echo "192.168.1.1"
exit 0
SH
    cat > "$SANDBOX/stub/netstat" <<'SH'
#!/bin/sh
# Claim Sockrocket's DNS port is listening, so update_dnsmasq proceeds.
echo "udp        0      0 127.0.0.1:5300           0.0.0.0:*"
SH
    cat > "$SANDBOX/stub/cru" <<'SH'
#!/bin/sh
echo "cru $*" >> "$SANDBOX/cru.calls"
exit 0
SH
    cat > "$SANDBOX/stub/service" <<'SH'
#!/bin/sh
exit 0
SH
    cat > "$SANDBOX/stub/ip" <<'SH'
#!/bin/sh
# `ip link show sockrocket-tun` succeeds only while the TUN marker exists.
case "$*" in
    *"link show"*) [ -f "$SANDBOX/tun_dev" ] && exit 0 || exit 1 ;;
esac
exit 1
SH
    cat > "$SANDBOX/stub/which" <<'SH'
#!/bin/sh
exit 0
SH
    cat > "$SANDBOX/stub/modprobe" <<'SH'
#!/bin/sh
echo "$*" >> "$SANDBOX/modprobe.calls"
exit 0
SH
    cat > "$SANDBOX/stub/insmod" <<'SH'
#!/bin/sh
exit 0
SH
    cat > "$SANDBOX/stub/mknod" <<'SH'
#!/bin/sh
exit 0
SH
    # iptables stand-in: sockrocket.sh probes the live SOCKROCKET_MANGLE chain from
    # do_status / do_proxy_off / do_watchdog. Model the chain as the same
    # marker file the iptables.sh stub maintains, so "chain holds a MARK
    # rule" is observable without touching the host firewall.
    cat > "$SANDBOX/stub/iptables" <<SH
#!/bin/sh
case "\$*" in
    *SOCKROCKET_MANGLE*)
        [ -f "$SANDBOX/mangle_chain" ] && echo "MARK all -- 0.0.0.0/0 0.0.0.0/0" || exit 1
        ;;
esac
exit 0
SH
    # fake ps for the orphan-cleanup tests: lists processes from ps.table and
    # hides entries whose pid has exited (mirrors /proc disappearing). The
    # default table is empty, so daemon_pids() is a no-op for every other
    # test �?real `ps` output on a dev box never contains the sandbox path.
    cat > "$SANDBOX/stub/ps" <<'SH'
#!/bin/sh
[ -f "$SANDBOX/ps.table" ] || exit 0
while read -r pid rest; do
    case "$pid" in ''|*[!0-9]*) continue ;; esac
    kill -0 "$pid" 2>/dev/null && echo "$pid $rest"
done < "$SANDBOX/ps.table"
SH
    : > "$SANDBOX/ps.table"

    chmod +x "$SANDBOX/stub/"*
    PATH="$SANDBOX/stub:$PATH"
    export PATH SANDBOX
}

teardown() {
    # do_start leaves the fake daemon and the fake API bridge running (both
    # are `sleep` copies with a 300s lifetime); without this each start-based
    # test leaks two processes that outlive their sandbox. `wait` then reaps
    # the ones this shell owns �?otherwise bash prints a "Killed" notice for
    # each into the test output, where it reads like a failure.
    local pids="" f pid
    [ -n "${FAKE_PID:-}" ] && pids="$FAKE_PID"
    if [ -n "${SOCKROCKET_DIR:-}" ]; then
        for f in "$SOCKROCKET_DIR/sockrocket.pid" "$SOCKROCKET_DIR/sockrocket-api.pid"; do
            [ -f "$f" ] || continue
            pid=$(cat "$f" 2>/dev/null)
            [ -n "$pid" ] && pids="$pids $pid"
        done
    fi
    for pid in $pids; do
        kill -9 "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
    done
    [ -n "${SANDBOX:-}" ] && rm -rf "$SANDBOX" 2>/dev/null
    FAKE_PID=""
}

# Launch the fake daemon so is_running() reports true.
spawn_daemon() {
    "$SOCKROCKET_DIR/sockrocket-cli" 300 >/dev/null 2>&1 &
    FAKE_PID=$!
    echo "$FAKE_PID" > "$SOCKROCKET_DIR/sockrocket.pid"
    sleep 1
}

# Run the sandbox copy (SOCKROCKET_DIR rewritten to the sandbox by setup()).
# SOCKROCKET_TUN_DEV is pinned for every action, not just ensure-tun: do_start and
# the watchdog now call ensure_tun themselves, and without the override the
# host's real /dev/net/tun would leak in (some hosts ship one) and make
# the proxy-wanted branches pass or fail for the wrong reason.
# SOCKROCKET_PROC_BASE points is_running at the comm files the fake daemon
# fabricates �?see make_fake_bin.
run_sockrocket() {
    SOCKROCKET_DNSMASQ_DIR="$DNS_DIR" SOCKROCKET_TUN_DEV="$SANDBOX/tun_dev" \
        SOCKROCKET_PROC_BASE="$PROC_FAKE" SOCKROCKET_RESOLV_FILE="$SANDBOX/resolv.dnsmasq" \
        SOCKROCKET_RESOLV_CONF="$SANDBOX/resolv.conf" \
        sh "$SANDBOX/sockrocket.sh" "$@" >/dev/null 2>&1
}

# Run the sandbox copy with the TUN marker pinned to the sandbox. Kept for
# the ensure-tun tests; run_sockrocket now passes the override for every action.
run_SOCKROCKET_SH() {
    SOCKROCKET_DNSMASQ_DIR="$DNS_DIR" SOCKROCKET_TUN_DEV="$SANDBOX/tun_dev" \
        SOCKROCKET_PROC_BASE="$PROC_FAKE" SOCKROCKET_RESOLV_FILE="$SANDBOX/resolv.dnsmasq" \
        SOCKROCKET_RESOLV_CONF="$SANDBOX/resolv.conf" \
        sh "$SANDBOX/sockrocket.sh" "$@" >/dev/null 2>&1
}

# ── Tests ─────────────────────────────────────────────────────────────────────

# Baseline: an ordinary stop with a live daemon must tear everything down.
test_stop_running_cleans_up() {
    printf '\n[1] stop while running: iptables + dnsmasq cleaned, cron removed\n'
    setup
    spawn_daemon
    touch "$SANDBOX/mangle_chain"

    run_sockrocket stop

    assert "iptables.sh stop invoked" "1" \
        "$(grep -c '^iptables.sh stop' "$SANDBOX/iptables.calls")"
    assert "mangle chain torn down" "gone" \
        "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    # Both SockrocketSubUpdate and SockrocketWatchdog are deleted �?two separate cru calls.
    assert "cron entries removed" "2" \
        "$(grep -c '^cru d' "$SANDBOX/cru.calls")"
    teardown
}

# THE REGRESSION: after a crash (TUN device missing / OOM-kill) the pid file
# points at a dead process. do_stop() used to `return 0` there, leaving the
# iptables MARK rules and the dnsmasq hijack (? catch-all -> 127.0.0.1:5300)
# active with nothing listening �?every LAN lookup then times out, and the UI
# offered no way to clear it.
test_stop_dead_daemon_still_cleans() {
    printf '\n[2] stop with a crashed daemon: cleanup must STILL run\n'
    setup
    echo "999999" > "$SOCKROCKET_DIR/sockrocket.pid"
    touch "$SANDBOX/mangle_chain"

    run_sockrocket stop

    assert "iptables.sh stop invoked despite dead daemon" "1" \
        "$(grep -c '^iptables.sh stop' "$SANDBOX/iptables.calls")"
    assert "mangle chain torn down" "gone" \
        "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    assert "stale pid file removed" "gone" \
        "$([ -f "$SOCKROCKET_DIR/sockrocket.pid" ] && echo present || echo gone)"
    teardown
}

# do_stop must record that the user asked for a stop, so the watchdog can
# tell a deliberate shutdown from a crash.
test_stop_records_intent() {
    printf '\n[3] stop records intent; start clears it\n'
    setup
    spawn_daemon
    run_sockrocket stop
    assert "stopped marker written" "present" \
        "$([ -f "$SOCKROCKET_DIR/stopped" ] && echo present || echo absent)"
    teardown
}

# Without the marker check the 5-minute cron watchdog resurrects the proxy and
# re-applies the hijack rules right after the user switches it off.
test_watchdog_respects_manual_stop() {
    printf '\n[4] watchdog must NOT resurrect a user-stopped service\n'
    setup
    echo "999999" > "$SOCKROCKET_DIR/sockrocket.pid"
    touch "$SOCKROCKET_DIR/stopped"

    run_sockrocket watchdog

    # do_start is what re-applies the hijack; assert it was never reached.
    # (grep -c prints 0 on no match AND exits 1, so a trailing `|| echo 0`
    # would emit the count twice �?count with awk instead.)
    assert "no iptables start" "0" \
        "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    assert "do_start not attempted" "0" \
        "$(awk '/^daemon\.info Starting Sockrocket\.\.\.$/{n++} END{print n+0}' "$SANDBOX/logger.calls")"
    teardown
}

# The marker must only suppress restarts after an EXPLICIT stop �?a genuine
# crash (no marker) still needs auto-recovery.
test_watchdog_recovers_crash() {
    printf '\n[5] watchdog still recovers an unexpected crash\n'
    setup
    echo "999999" > "$SOCKROCKET_DIR/sockrocket.pid"
    rm -f "$SOCKROCKET_DIR/stopped"

    run_sockrocket watchdog

    # do_start logs "Starting Sockrocket..." exactly once (the failure tail re-logs
    # it with a different prefix, so anchor to the daemon.info line).
    assert "watchdog attempted a restart" "1" \
        "$(awk '/^daemon\.info Starting Sockrocket\.\.\.$/{n++} END{print n+0}' "$SANDBOX/logger.calls")"
    teardown
}

# Belt and braces: the fail-open check must drop the dnsmasq hijack when the
# daemon is confirmed DEAD (crash recovery attempted and failed) and the
# hijack file is still installed. A live daemon with a flaky port reading is
# a different case �?covered by the threshold test below.
test_no_dns_hijack_without_daemon() {
    printf '\n[6] daemon dead (unrecoverable) with installed hijack: watchdog removes it\n'
    setup
    touch "$DNS_DIR/sockrocket.conf"   # hijack installed...
    echo "999999" > "$SOCKROCKET_DIR/sockrocket.pid"   # ...but daemon is dead
    rm -f "$SOCKROCKET_DIR/sockrocket-cli"    # ...and unrecoverable ("Binary not found")
    # netstat stub claims port 5300 is listening �?silence it so the hijack
    # really dangles.
    cat > "$SANDBOX/stub/netstat" <<'SH'
#!/bin/sh
exit 1
SH
    chmod +x "$SANDBOX/stub/netstat"

    run_sockrocket watchdog

    # Crash recovery runs do_start (no marker); with the binary gone the
    # daemon stays dead �?fail-open removes the dangling hijack immediately,
    # without waiting for the consecutive-failure threshold.
    assert "dangling hijack removed" "gone" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# ensure_tun: already-present device is a no-op (idempotent).
test_ensure_tun_idempotent() {
    printf '\n[7] ensure_tun: device already present -> no modprobe call\n'
    setup
    touch "$SANDBOX/tun_dev"          # /dev/net/tun stands in
    run_SOCKROCKET_SH "ensure-tun"
    assert "no modprobe invoked" "0" "$(cat "$SANDBOX/modprobe.calls" 2>/dev/null | wc -l | tr -d ' ')"
    teardown
}

# ensure_tun: modprobe fails but insmod succeeds -> returns 0.
test_ensure_tun_insmod_fallback() {
    printf '\n[8] ensure_tun: modprobe fails, insmod fallback succeeds\n'
    setup
    rm -f "$SANDBOX/tun_dev"
    # modprobe is stubbed to fail and record the call.
    printf '#!/bin/sh\necho "$*" >> "$SANDBOX/modprobe.calls"\nexit 1\n' > "$SANDBOX/stub/modprobe"
    chmod +x "$SANDBOX/stub/modprobe"
    # insmod succeeds and creates the device marker.
    printf '#!/bin/sh\necho "$*" >> "$SANDBOX/insmod.calls"\ntouch "$SANDBOX/tun_dev"\nexit 0\n' > "$SANDBOX/stub/insmod"
    chmod +x "$SANDBOX/stub/insmod"
    # Pin the kernel version: a hardcoded insmod path would still match the
    # generic pattern, so the assertion below is only meaningful if `uname -r`
    # yields this stubbed value.
    printf '#!/bin/sh\necho "9.9.9-test"\n' > "$SANDBOX/stub/uname"
    chmod +x "$SANDBOX/stub/uname"
    run_SOCKROCKET_SH "ensure-tun"
    assert "modprobe was tried" "1" "$(wc -l < "$SANDBOX/modprobe.calls" | tr -d ' ')"
    assert "insmod fallback was used" "1" "$(wc -l < "$SANDBOX/insmod.calls" | tr -d ' ')"
    assert "insmod path uses uname -r" "1" \
        "$(grep -c '/lib/modules/9\.9\.9-test/kernel/drivers/net/tun\.ko' "$SANDBOX/insmod.calls")"
    assert "success path logged" "1" "$(grep -c 'TUN device ready' "$SANDBOX/logger.calls")"
    teardown
}

# ensure_tun: all three levels fail -> the dispatch branch logs "TUN
# unavailable". The subcommand itself always exits 0 (`|| true` in the
# dispatch branch), so the failure path is asserted via the logger stub.
test_ensure_tun_total_failure() {
    printf '\n[9] ensure_tun: all fallbacks fail -> returns non-zero\n'
    setup
    rm -f "$SANDBOX/tun_dev"
    for c in modprobe insmod mknod; do
        printf '#!/bin/sh\nexit 1\n' > "$SANDBOX/stub/$c"
        chmod +x "$SANDBOX/stub/$c"
    done
    run_SOCKROCKET_SH "ensure-tun"
    assert "failure path logged" "1" "$(grep -c 'TUN unavailable' "$SANDBOX/logger.calls")"
    teardown
}

# transparent_proxy=false -> no --tun, no iptables rules.
test_proxy_off_skips_tun_and_iptables() {
    printf '\n[10] transparent_proxy=false: no --tun, no iptables\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: false/" "$SOCKROCKET_DIR/config.yaml"
    sed -i "s/^dns_hijack:.*/dns_hijack: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/tun_dev"
    run_sockrocket start
    assert "no iptables start" "0" "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    assert "no dnsmasq hijack" "gone" "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# transparent_proxy=true + dns_hijack=false -> rules yes, hijack no.
test_proxy_only() {
    printf '\n[11] transparent_proxy=true, dns_hijack=false: rules yes, hijack no\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: true/" "$SOCKROCKET_DIR/config.yaml"
    sed -i "s/^dns_hijack:.*/dns_hijack: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/tun_dev"
    run_sockrocket start
    assert "iptables start invoked" "1" "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    assert "dns hijack NOT written" "gone" "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# dns_hijack=true but the DNS port is not listening -> refuse (no blackhole).
test_dns_on_refuses_without_listener() {
    printf '\n[12] dns-on with no listener on the DNS port: refused\n'
    setup
    printf '#!/bin/sh\nexit 1\n' > "$SANDBOX/stub/netstat"   # nothing listening
    chmod +x "$SANDBOX/stub/netstat"
    run_sockrocket dns-on
    assert "sockrocket.conf NOT written" "gone" "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# proxy-off tears down rules but leaves the daemon running.
test_proxy_off_keeps_daemon() {
    printf '\n[13] proxy-off: rules down, daemon untouched\n'
    setup
    spawn_daemon
    touch "$SANDBOX/mangle_chain"
    local pid_before; pid_before=$(cat "$SOCKROCKET_DIR/sockrocket.pid")
    run_sockrocket proxy-off
    assert "iptables stop invoked" "1" "$(grep -c '^iptables.sh stop' "$SANDBOX/iptables.calls")"
    assert "mangle chain gone" "gone" "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    assert "daemon PID unchanged" "$pid_before" "$(cat "$SOCKROCKET_DIR/sockrocket.pid" 2>/dev/null)"
    # The toggles are independent: killing the proxy must NOT drop the DNS
    # hijack (test [11] covers the reverse direction).
    assert "dns hijack left alone" "present" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# Legacy config (no toggle keys, mode: tun) derives both true.
test_legacy_mode_tun_derives_both_on() {
    printf '\n[14] legacy config: mode=tun derives both toggles true\n'
    setup
    grep -v "^transparent_proxy:\|^dns_hijack:" "$SOCKROCKET_DIR/config.yaml" > "$SOCKROCKET_DIR/c2" && mv "$SOCKROCKET_DIR/c2" "$SOCKROCKET_DIR/config.yaml"
    sed -i 's/^mode:.*/mode: "tun"/' "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/tun_dev"
    # setup() pre-creates the hijack file, so its presence afterwards would
    # prove nothing; drop it and let do_start re-install it.
    rm -f "$DNS_DIR/sockrocket.conf"
    run_sockrocket start
    assert "iptables start invoked" "1" "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    assert "dns hijack written" "present" "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# dns_hijack=false but a stale hijack file survived: the watchdog removes it.
# Without this a user who turned DNS hijack off would keep the LAN pointed at
# a DNS port that may not even be listening.
test_watchdog_clears_stale_hijack_when_disabled() {
    printf '\n[15] watchdog: dns_hijack=false + stale sockrocket.conf -> removed\n'
    setup
    sed -i "s/^dns_hijack:.*/dns_hijack: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$DNS_DIR/sockrocket.conf"          # leftover from a previous run
    spawn_daemon                        # service is up, so no crash recovery
    run_sockrocket watchdog
    assert "stale hijack removed" "gone" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# dns-off removes the hijack file; the reverse of test [12], and the only
# brief-listed subcommand that had no coverage at all.
test_dns_off_removes_hijack() {
    printf '\n[17] dns-off: hijack file removed\n'
    setup
    touch "$DNS_DIR/sockrocket.conf"
    run_sockrocket dns-off
    assert "sockrocket.conf removed" "gone" "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    teardown
}

# transparent_proxy=true but the TUN device vanished (reboot): the watchdog
# reloads the module and re-applies the rules without user intervention.
test_watchdog_selfheals_tun() {
    printf '\n[16] watchdog: TUN missing while proxying -> reload + reapply\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: true/" "$SOCKROCKET_DIR/config.yaml"
    rm -f "$SANDBOX/tun_dev"            # device gone after a reboot
    spawn_daemon
    printf '#!/bin/sh\necho "$*" >> "$SANDBOX/modprobe.calls"\ntouch "$SANDBOX/tun_dev"\nexit 0\n' > "$SANDBOX/stub/modprobe"
    chmod +x "$SANDBOX/stub/modprobe"
    run_sockrocket watchdog
    assert "ensure_tun reloaded the module" "1" "$(wc -l < "$SANDBOX/modprobe.calls" | tr -d ' ')"
    assert "iptables rules reapplied" "1" "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    teardown
}

# transparent_proxy=false: the watchdog must NOT reapply proxy rules, even
# when the TUN device is present and the rules are missing. Without the
# guard the rules silently come back and LAN traffic enters a dead TUN.
test_watchdog_skips_proxy_when_disabled() {
    printf '\n[18] watchdog: transparent_proxy=false -> never reapplies rules\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/tun_dev"          # TUN present, so only the key can stop it
    rm -f "$SANDBOX/mangle_chain"     # rules missing
    spawn_daemon
    run_sockrocket watchdog
    assert "iptables start NOT invoked" "0" \
        "$(grep -c '^iptables.sh start' "$SANDBOX/iptables.calls")"
    teardown
}

# A stale MARK chain with the proxy switched off must be torn down, exactly
# like the DNS hijack equivalent. Otherwise LAN traffic keeps entering a TUN
# that may no longer exist.
test_watchdog_clears_stale_mark_when_proxy_disabled() {
    printf '\n[19] watchdog: transparent_proxy=false + stale MARK -> removed\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/mangle_chain"     # stale rules from a crashed run
    spawn_daemon
    run_sockrocket watchdog
    assert "stale MARK chain removed" "gone" \
        "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    teardown
}

# do_start must also clear a stale MARK chain when the proxy key is off.
test_start_clears_stale_mark_when_proxy_disabled() {
    printf '\n[20] start: transparent_proxy=false + stale MARK -> removed\n'
    setup
    sed -i "s/^transparent_proxy:.*/transparent_proxy: false/" "$SOCKROCKET_DIR/config.yaml"
    sed -i "s/^dns_hijack:.*/dns_hijack: false/" "$SOCKROCKET_DIR/config.yaml"
    touch "$SANDBOX/mangle_chain"
    rm -f "$DNS_DIR/sockrocket.conf"
    run_sockrocket start
    assert "stale MARK chain removed" "gone" \
        "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    teardown
}

# THE ORPHAN BUG (seen live on the router): the pid file pointed at a dead
# process while a daemon started outside it still held ports 1080/1087/5300.
# Every restart then killed the dead pid, launched a duplicate that died with
# EADDRINUSE, and the watchdog respawned another corpse every 5 minutes �?# while status reported "stopped" and the orphan kept serving stale config.
# do_start must kill untracked daemons BEFORE launching the new one.
test_start_kills_orphan_daemon() {
    printf '\n[21] start: untracked orphan daemon -> killed, new daemon tracked\n'
    setup
    "$SOCKROCKET_DIR/sockrocket-cli-bin" 300 >/dev/null 2>&1 &
    orphan=$!
    echo "$orphan root 0:00 $SOCKROCKET_DIR/sockrocket-cli $SOCKROCKET_DIR/config.yaml --tun" \
        >> "$SANDBOX/ps.table"
    run_sockrocket start
    assert "orphan daemon terminated" "dead" \
        "$(kill -0 "$orphan" 2>/dev/null && echo alive || echo dead)"
    assert "pid file tracks the new daemon" "present" \
        "$([ -f "$SOCKROCKET_DIR/sockrocket.pid" ] && echo present || echo gone)"
    wait "$orphan" 2>/dev/null
    teardown
}

# Same orphan, other entry point: stop with a stale pid file must ALSO kill
# the untracked daemon �?otherwise "stop" leaves a proxy running that nothing
# can see or manage.
test_stop_kills_orphan_daemon() {
    printf '\n[22] stop: stale pid file + orphan daemon -> orphan killed too\n'
    setup
    "$SOCKROCKET_DIR/sockrocket-cli-bin" 300 >/dev/null 2>&1 &
    orphan=$!
    echo "$orphan root 0:00 $SOCKROCKET_DIR/sockrocket-cli $SOCKROCKET_DIR/config.yaml --tun" \
        >> "$SANDBOX/ps.table"
    echo "999999" > "$SOCKROCKET_DIR/sockrocket.pid"   # dead pid, as after a crash
    touch "$SANDBOX/mangle_chain"

    run_sockrocket stop

    assert "orphan daemon terminated" "dead" \
        "$(kill -0 "$orphan" 2>/dev/null && echo alive || echo dead)"
    assert "mangle chain torn down" "gone" \
        "$([ -f "$SANDBOX/mangle_chain" ] && echo present || echo gone)"
    wait "$orphan" 2>/dev/null
    teardown
}

# THE REGRESSION: pin_dns_upstream overwrote the firmware's
# /tmp/resolv.dnsmasq with the Sockrocket listener but no stop path ever restored
# it �?the original comment assumed the firmware regenerates the file on
# dnsmasq restart, which current Merlin does NOT do. Every Sockrocket stop left
# dnsmasq pointing at a dead 127.0.0.1:5300 and the whole LAN lost DNS.
test_dns_unpin_on_stop() {
    printf '\n[23] start pins upstream, stop restores ISP DNS (unpin)\n'
    setup

    run_sockrocket start
    assert "upstream pinned to Sockrocket listener while running" \
        "server=127.0.0.1#5300" "$(cat "$SANDBOX/resolv.dnsmasq")"
    assert "ISP upstream backed up" "present" \
        "$([ -f "$SANDBOX/resolv.dnsmasq.Sockrocket-bak" ] && echo present || echo gone)"

    run_sockrocket stop
    assert "ISP upstream restored after stop" "server=192.168.1.1" \
        "$(cat "$SANDBOX/resolv.dnsmasq")"
    assert "backup consumed" "gone" \
        "$([ -f "$SANDBOX/resolv.dnsmasq.Sockrocket-bak" ] && echo present || echo gone)"
    teardown
}

# Restart while pinned must not snapshot the pin as the "original" �?# otherwise the backup would hold the dead listener address and every later
# stop would restore a broken upstream.
test_dns_repin_never_backs_up_pin() {
    printf '\n[24] double start keeps the ORIGINAL backup (no pin snapshot)\n'
    setup

    run_sockrocket start
    run_sockrocket stop
    # Simulate a crash-era leftover: pinned file but no backup (the old build's
    # state). A fresh start must treat it as pinned, not as ISP content.
    echo "server=127.0.0.1#5300" > "$SANDBOX/resolv.dnsmasq"
    run_sockrocket start
    run_sockrocket stop
    assert "no backup: rebuilt from system resolver" "server=192.168.1.1" \
        "$(cat "$SANDBOX/resolv.dnsmasq")"
    teardown
}

# A busybox netstat snapshot can lie (format quirks, a race with a daemon
# restart), and one bad reading used to tear the whole hijack down. With a
# LIVE daemon the hijack must survive isolated probe failures and only fail
# open after 3 consecutive ones.
test_watchdog_dns_port_failure_threshold() {
    printf '\n[25] watchdog: live daemon + dead DNS port -> fail-open only after 3 consecutive checks\n'
    setup
    spawn_daemon
    cat > "$SANDBOX/stub/netstat" <<'SH'
#!/bin/sh
exit 1
SH
    chmod +x "$SANDBOX/stub/netstat"

    run_sockrocket watchdog
    assert "hijack survives 1st failure" "present" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    assert "failure counted (1)" "1" "$(cat "$SOCKROCKET_DIR/.wdns_fails" 2>/dev/null)"

    run_sockrocket watchdog
    assert "hijack survives 2nd failure" "present" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    assert "failure counted (2)" "2" "$(cat "$SOCKROCKET_DIR/.wdns_fails" 2>/dev/null)"

    run_sockrocket watchdog
    assert "hijack removed on 3rd consecutive failure" "gone" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    assert "failure counter reset after teardown" "" \
        "$(cat "$SOCKROCKET_DIR/.wdns_fails" 2>/dev/null)"
    teardown
}

# ...and a recovery between failures resets the counter, so one bad reading
# never compounds into a teardown.
test_watchdog_dns_failure_counter_resets() {
    printf '\n[26] watchdog: port probe recovering resets the failure counter\n'
    setup
    spawn_daemon
    cat > "$SANDBOX/stub/netstat" <<'SH'
#!/bin/sh
exit 1
SH
    chmod +x "$SANDBOX/stub/netstat"

    run_sockrocket watchdog
    run_sockrocket watchdog
    assert "counter at 2 after two failures" "2" \
        "$(cat "$SOCKROCKET_DIR/.wdns_fails" 2>/dev/null)"

    # Port "recovers" (the default stub claims :5300 is listening again).
    cat > "$SANDBOX/stub/netstat" <<'SH'
#!/bin/sh
echo "udp        0      0 127.0.0.1:5300           0.0.0.0:*"
exit 0
SH
    chmod +x "$SANDBOX/stub/netstat"

    run_sockrocket watchdog
    assert "hijack still installed" "present" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    assert "counter reset after recovery" "" \
        "$(cat "$SOCKROCKET_DIR/.wdns_fails" 2>/dev/null)"
    teardown
}

# Regression: sockrocket.conf disappeared while the daemon was healthy
# and nothing put it back �?LAN DNS silently fell back to the GFW-poisoned
# ISP path. The watchdog must now REINSTALL the hijack, not just refrain
# from removing it.
test_watchdog_selfheals_missing_hijack() {
    printf '\n[27] watchdog: daemon alive + sockrocket.conf missing -> hijack reinstalled\n'
    setup
    spawn_daemon
    rm -f "$DNS_DIR/sockrocket.conf"

    run_sockrocket watchdog

    assert "hijack reinstalled" "present" \
        "$([ -f "$DNS_DIR/sockrocket.conf" ] && echo present || echo gone)"
    assert "upstream pinned by self-heal" "server=127.0.0.1#5300" \
        "$(cat "$SANDBOX/resolv.dnsmasq")"
    teardown
}

printf '=== sockrocket.sh stop/watchdog semantics ===\n'
test_stop_running_cleans_up
test_stop_dead_daemon_still_cleans
test_stop_records_intent
test_watchdog_respects_manual_stop
test_watchdog_recovers_crash
test_no_dns_hijack_without_daemon
test_ensure_tun_idempotent
test_ensure_tun_insmod_fallback
test_ensure_tun_total_failure
test_proxy_off_skips_tun_and_iptables
test_proxy_only
test_dns_on_refuses_without_listener
test_proxy_off_keeps_daemon
test_legacy_mode_tun_derives_both_on
test_watchdog_clears_stale_hijack_when_disabled
test_watchdog_selfheals_tun
test_watchdog_skips_proxy_when_disabled
test_watchdog_clears_stale_mark_when_proxy_disabled
test_start_clears_stale_mark_when_proxy_disabled
test_dns_off_removes_hijack
test_start_kills_orphan_daemon
test_stop_kills_orphan_daemon
test_dns_unpin_on_stop
test_dns_repin_never_backs_up_pin
test_watchdog_dns_port_failure_threshold
test_watchdog_dns_failure_counter_resets
test_watchdog_selfheals_missing_hijack

printf '\n=== %d passed, %d failed ===\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]

