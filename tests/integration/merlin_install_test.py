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
               SOCKROCKET_DIR=str(root), SOCKROCKET_SCRIPTS=str(root))
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
    finally:
        old.terminate()
        old.wait()
print('PASS: atomic upgrade of a running executable, invalid-package rollback, API restart and preserved config/Stop')
