"""Package upgrades preserve executable availability and existing user settings."""
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile

source = Path('merlin/install.sh').read_text()
helper = source[source.index('install_binary() {'):source.index('if [ "$UPDATE_ONLY" = "true" ]; then')]
startup = source[source.index('post_install_start() {'):source.index('# koolshare / koolcenter install path')]
defaults = source[source.index('install_default_config() {'):source.index('install_common_files() {')]
with tempfile.TemporaryDirectory(prefix='sockrocket-install-') as directory:
    root = Path(directory)
    installed = root / 'sockrocket-cli'
    bundled = root / 'bundled'
    config = root / 'config.yaml'
    config.write_bytes(b'fixture configuration retained\n')
    (root / 'stopped').touch()
    if sys.platform == 'darwin':
        # Apple platform-signed tools cannot execute from a copied path.
        installed.write_text('#!/bin/sh\nexec sleep "$1"\n')
    else:
        # Linux holds this actual ELF inode open; in-place cp would fail ETXTBSY.
        shutil.copyfile(shutil.which('sleep'), installed)
    installed.chmod(0o700)
    old = subprocess.Popen([str(installed), '30'])
    env = dict(os.environ, SOCKROCKET_BIN=str(installed), BINARY=str(bundled),
               SOCKROCKET_DIR=str(root), SOCKROCKET_SCRIPTS=str(root),
               SOCKROCKET_CONF=str(config), SCRIPT_DIR=str(root))
    def run(script, check=True):
        return subprocess.run(['sh', '-c', 'xcp() { cp "$1" "$2"; }\nerr() { echo "$*" >&2; exit 1; }\n' + script], env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=check)
    try:
        bundled.write_text('#!/bin/sh\n[ "$1" = --version ] && echo "sockrocket-cli fixture"\n')
        bundled.chmod(0o700)
        run(helper + '\ninstall_binary\n')
        assert old.poll() is None, 'running executable must survive atomic replacement'
        assert subprocess.check_output([str(installed), '--version']).strip() == b'sockrocket-cli fixture'
        good = installed.read_bytes()
        bundled.write_text('#!/bin/sh\nexit 1\n')
        assert run(helper + '\ninstall_binary\n', check=False).returncode != 0
        assert installed.read_bytes() == good, 'invalid package must retain installed binary'
        assert not Path(str(installed) + '.install-new').exists()
        assert config.read_bytes() == b'fixture configuration retained\n'
        assert (root / 'stopped').exists()
        control = root / 'sockrocket.sh'
        control.write_text('#!/bin/sh\necho "$1" >> "$SOCKROCKET_DIR/calls"\n')
        control.chmod(0o700)
        run('sleep() { :; }\nhave() { [ "$1" = cru ]; }\ncru() { echo "$*" > "$SOCKROCKET_DIR/cron"; }\ninfo() { :; }\nok() { :; }\nwarn() { :; }\n' + startup + '\npost_install_start\nwait\n')
        calls = (root / 'calls').read_text().splitlines()
        assert calls == ['api-restart', 'restart'], 'both processes must use the new binary without overriding manual Stop'
        cron = (root / 'cron').read_text()
        assert 'sockrocket.sh restart;' in cron and 'sockrocket.sh start;' not in cron
        (root / 'config.yaml.template').write_bytes(b'fresh default configuration\n')
        prelude = 'info() { :; }\nok() { :; }\n'
        # Upgrade preserves both a running user's intent and an explicit Stop.
        (root / 'stopped').unlink()
        run(prelude + defaults + '\ninstall_default_config\n')
        assert not (root / 'stopped').exists()
        assert config.read_bytes() == b'fixture configuration retained\n'
        (root / 'stopped').touch()
        run(prelude + defaults + '\ninstall_default_config\n')
        assert (root / 'stopped').exists()
        # Fresh install has no proxy start intent, even after delayed retries.
        config.unlink()
        (root / 'stopped').unlink()
        run(prelude + defaults + '\ninstall_default_config\n')
        assert config.read_bytes() == b'fresh default configuration\n'
        assert (root / 'stopped').exists()
        # A mounted JFFS directory is insufficient when Merlin has disabled
        # execution of custom scripts; fail before publishing hook-dependent files.
        jffs_guard = source[source.index('# Merlin only dispatches /jffs/scripts/*'):
                            source.index('# ── Shared helpers', source.index('# Merlin only dispatches /jffs/scripts/*'))]
        assert run('have() { :; }\nnvram() { echo 0; }\n' + jffs_guard, check=False).returncode != 0
        run('have() { :; }\nnvram() { echo 1; }\n' + jffs_guard)
        # Exercise the installed boot wrapper, not a hand-written duplicate.
        a = source.index("    printf '%s\\n' \\\n", source.index('# koolshare startup wrapper'))
        b = source.index('    chmod +x "$KS_SCRIPTS/${MODULE}_config.sh"', a)
        wrapper = source[a:b].replace('/jffs/addons/sockrocket/scripts/sockrocket.sh', str(control))
        wrapper_env = dict(env, KS_SCRIPTS=str(root), MODULE='sockrocket')
        subprocess.run(['sh', '-c', wrapper], env=wrapper_env, check=True)
        (root / 'calls').unlink()
        subprocess.run(['sh', str(root / 'sockrocket_config.sh'), 'start'], env=env, check=True)
        assert (root / 'calls').read_text().splitlines() == ['boot']
        stubs = root / 'stubs'
        stubs.mkdir()
        (stubs / 'dbus').write_text('#!/bin/sh\nprintf "%s" "$TEST_PROXY_ENABLED"\n')
        (stubs / 'curl').write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$SOCKROCKET_DIR/response"\n')
        for stub in stubs.iterdir():
            stub.chmod(0o700)
        for enabled, expected in [('0', 'stop'), ('1', 'start')]:
            (root / 'calls').unlink()
            requested = dict(env, PATH=str(stubs) + os.pathsep + env['PATH'], TEST_PROXY_ENABLED=enabled)
            subprocess.run(['sh', str(root / 'sockrocket_config.sh'), '321'], env=requested, check=True)
            assert (root / 'calls').read_text().splitlines() == [expected]
            assert '--data 321 http://127.0.0.1:3030/_resp/321' in (root / 'response').read_text()
        status = Path('merlin/scripts/sockrocket_status.sh').read_text().replace(
            '/jffs/addons/sockrocket/scripts/sockrocket.sh', str(control))
        status_path = root / 'sockrocket_status.sh'
        status_path.write_text(status)
        control.write_text('#!/bin/sh\nprintf "Sockrocket stopped\\n"\n')
        response = subprocess.check_output(['sh', str(status_path)], env=env).decode().strip()
        assert response == 'Sockrocket stopped' and 'PID' not in response
        subprocess.run(['sh', str(status_path), '123'], env=requested, check=True)
        assert '--data Sockrocket stopped http://127.0.0.1:3030/_resp/123' in (root / 'response').read_text()

        # Install the actual Merlin hook bodies in a sandbox. This caught a
        # shift-2 regression that wrote marker-only, nonfunctional scripts.
        hook_dir = root / 'hooks'
        hook_dir.mkdir()
        (hook_dir / 'wan-event').write_text('#!/bin/sh\n: # another add-on hook\n')
        control.write_text('#!/bin/sh\nprintf "%s\\n" "$1" >> "$SOCKROCKET_DIR/hook-calls"\n')
        control.chmod(0o700)
        hook_helper = source[source.index('install_hook() {'):source.index('# Bring up API')]
        wan_helper = source[source.index('install_wan_hooks() {'):source.index('ks_install() {')]
        hook_calls = source[source.index('    install_hook "/jffs/scripts/firewall-start"'):source.index('    # Post-install validation')]
        hook_script = (wan_helper + hook_calls).replace('/jffs/scripts/', str(hook_dir) + '/')
        hook_script = hook_script.replace('/jffs/addons/sockrocket/scripts/sockrocket.sh', str(control))
        assert source.count('    install_wan_hooks\n') == 2, 'both standard and koolcenter installs need WAN hooks'
        run('ok() { :; }\n' + hook_helper + hook_script)
        run('ok() { :; }\n' + hook_helper + hook_script)
        assert ': # another add-on hook' in (hook_dir / 'wan-event').read_text()

        def event(name, *args):
            subprocess.run(['sh', str(hook_dir / name), *args], env=env, check=True)

        event('wan-event', '0', 'connecting')
        event('wan-event', '0', 'connected')
        event('wan-event', '1', 'disconnected')
        event('wan-start')
        event('dhcpc-event', 'renew')
        event('dhcpc-event', 'bound')
        event('dhcpc-event', 'renew', '4')
        event('dhcpc-event', 'bound', '6')
        event('dhcpc-event', 'unbound', '6')
        event('dhcpc-event', 'stopped', '6')
        event('dhcpc-event', 'updated', '6')
        event('dhcpc-event', 'rebound', '6')
        event('dhcpc-event', 'ra-updated', '6')
        event('dhcpc-event', 'deconfig')
        event('firewall-start', 'ppp0')
        event('nat-start')
        event('services-start')
        event('service-event', 'restart', 'Sockrocket')
        assert (root / 'hook-calls').read_text().splitlines() == [
            'wan-restore', 'wan-restore', 'pin-dns', 'pin-dns', 'pin-dns',
            'dhcpv6-restore', 'dhcpv6-restore', 'dhcpv6-restore',
            'dhcpv6-restore', 'dhcpv6-restore', 'dhcpv6-restore',
            'firewall-restore', 'nat-restore', 'boot', 'restart',
        ], 'WAN and DHCPv4/DHCPv6 events must dispatch once after idempotent installation'
    finally:
        old.terminate()
        old.wait()
print('PASS: atomic upgrade, preserved install intent, boot wrapper and WAN event hooks')
