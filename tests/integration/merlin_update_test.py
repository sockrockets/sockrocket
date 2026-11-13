"""Exercise durable recovery without a router or root filesystem changes."""
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import tarfile
import unittest

WORKER = pathlib.Path(__file__).resolve().parents[2] / 'merlin/scripts/update.sh'

class RecoveryTest(unittest.TestCase):
    def run_recovery(self, root, unhealthy=False, offline=False):
        tools = root / '.test-tools'
        tools.mkdir(exist_ok=True)
        for name, body in {'curl': "echo '{\"version\":\"test-version\"}'", 'nslookup': 'exit 0'}.items():
            path = tools / name
            if unhealthy:
                body = 'exit 7'
            elif offline and name == 'curl':
                body = 'case "$*" in *--socks5-hostname*) exit 7;; esac\n' + body
            path.write_text('#!/bin/sh\n' + body + '\n')
            path.chmod(0o755)
        if not (root / 'config.yaml').exists(): (root / 'config.yaml').write_text('socks_port: 1080\n')
        (root / 'api-token').write_text('test-token')
        (root / 'sockrocket.pid').write_text(str(os.getpid()))
        return subprocess.run(['sh', str(WORKER), '--recover'],
            env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                 'PATH': str(tools) + os.pathsep + os.environ['PATH']}, capture_output=True, timeout=15)

    def test_interrupted_publish_restores_by_rename(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            (tx / 'old/scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (tx / 'old/sockrocket-cli').write_text('old')
            (tx / 'old/version').write_text('test-version')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('0')
            (root / 'sockrocket-cli').write_text('new')
            result = self.run_recovery(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
            self.assertFalse(tx.exists())
            self.assertEqual(self.run_recovery(root).returncode, 0)

    def test_offline_rollback_after_reboot_releases_transaction(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            (tx / 'old/scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (tx / 'old/version').write_text('test-version')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('1')
            # A previous boot left the legacy persistent recovery lock behind.
            (root / '.update-restore-lock').mkdir()
            result = self.run_recovery(root, offline=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(tx.exists())
            self.assertTrue((root / 'update-status').read_text().startswith('failed: 已恢复旧版'))

    def test_compressed_backup_recovers_and_can_retry(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (root / 'scripts').mkdir()
            (root / 'scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (root / 'sockrocket-cli').write_text('old')
            (root / 'version').write_text('test-version')
            tx.mkdir()
            (root / 'config.yaml').write_text('original configuration')
            (root / 'config.yaml.bak').write_text('original configuration backup')
            (tx / 'config-backed-up').touch()
            with tarfile.open(tx / 'backup.tar.gz', 'w:gz') as archive:
                archive.add(root / 'sockrocket-cli', arcname='sockrocket-cli')
                archive.add(root / 'scripts', arcname='scripts')
                archive.add(root / 'version', arcname='version')
                archive.add(root / 'config.yaml', arcname='config.yaml')
                archive.add(root / 'config.yaml.bak', arcname='config.yaml.bak')
            (root / 'config.yaml').write_text('new version changed configuration')
            (root / 'config.yaml.bak').write_text('new backup')
            (root / 'sockrocket-cli').write_text('new')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('0')
            result = self.run_recovery(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
            self.assertFalse(tx.exists())
            self.assertEqual((root / 'config.yaml').read_text(), 'original configuration')
            self.assertEqual((root / 'config.yaml.bak').read_text(), 'original configuration backup')

    def test_second_recovery_cannot_modify_installation(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            tx.mkdir()
            (tx / 'prepared').touch()
            (root / 'runtime/.update-restore-lock').mkdir(parents=True)
            (root / 'sockrocket-cli').write_text('running installation')
            result = self.run_recovery(root)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'running installation')
            self.assertTrue(tx.exists())
            self.assertFalse((tx / 'owner').exists())

    def test_failed_health_retains_backup_after_successful_start(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            (tx / 'old/scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (tx / 'old/version').write_text('test-version')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('0')
            result = self.run_recovery(root, unhealthy=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue(tx.exists())
            self.assertFalse((root / 'runtime/.update-restore-lock').exists())

    def test_corrupt_backup_does_not_delete_installed_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            tx.mkdir()
            (tx / 'backup.tar.gz').write_bytes(b'broken')
            (tx / 'prepared').touch()
            (root / 'sockrocket-cli').write_text('new')
            self.assertNotEqual(self.run_recovery(root).returncode, 0)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'new')
            self.assertTrue(tx.exists())

    def test_nested_recovery_is_suppressed_and_interfaces_restore_first(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            external = root / 'external-script'
            external.write_text('new')
            saved = tx / 'external' / str(external).lstrip('/')
            saved.parent.mkdir(parents=True)
            saved.write_text('old')
            (tx / 'external-list').write_text(str(external) + '\n')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('1')
            (tx / 'old/version').write_text('test-version')
            script = tx / 'old/scripts/sockrocket.sh'
            script.write_text('#!/bin/sh\n'
                'test "$1" = stop && exit 0\n'
                'test "$(cat "$SOCKROCKET_DIR/external-script")" = old || exit 9\n'
                'sh "' + str(WORKER) + '" --recover\n')
            result = self.run_recovery(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(tx.exists())
            self.assertEqual(external.read_text(), 'old')

    def test_insufficient_space_leaves_install_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = pathlib.Path(tmp)
            root, pkg, work, commands = [base / n for n in ('root', 'pkg', 'work', 'commands')]
            for path in (root, pkg, work, commands):
                path.mkdir()
            (root / 'sockrocket-cli').write_text('old')
            for name in ('bin', 'scripts', 'webui', 'res'):
                (pkg / name).mkdir()
            for name in ('version', 'uninstall.sh'):
                (pkg / name).touch()
            # Same-size upgrade: the old binary can still be mapped.
            data = b'x' * (2 * 1024 * 1024)
            (root / 'sockrocket-cli').write_bytes(data)
            (pkg / 'bin/sockrocket-cli').write_bytes(data)
            df = commands / 'df'
            df.write_text('#!/bin/sh\nprintf "Filesystem 1024-blocks Used Available Capacity Mounted\\nmock 10000 5000 5000 50%% /\\n"\n')
            df.chmod(0o755)
            result = subprocess.run(['sh', str(WORKER), str(pkg), str(work), '--check-space'],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            message = result.stderr.decode()
            match = re.search(r'需要 (\d+) KiB，可用 5000 KiB，还差 (\d+) KiB', message)
            self.assertIsNotNone(match, message)
            required, shortfall = map(int, match.groups())
            self.assertEqual(shortfall, required - 5000)
            self.assertFalse((root / 'update-status').exists())
            self.assertEqual((root / 'sockrocket-cli').read_bytes(), data)
            self.assertFalse((root / '.update-transaction').exists())

    def test_incompatible_configuration_rejected_before_transaction(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = pathlib.Path(tmp)
            root, pkg, work, commands = [base / n for n in ('root', 'pkg', 'work', 'commands')]
            for path in (root, pkg, work, commands): path.mkdir()
            (root / 'sockrocket-cli').write_text('old')
            (root / 'config.yaml').write_text('legacy config remains private')
            for name in ('bin', 'scripts', 'webui', 'res'): (pkg / name).mkdir()
            for name in ('version', 'uninstall.sh'): (pkg / name).touch()
            binary = pkg / 'bin/sockrocket-cli'
            binary.write_text('#!/bin/sh\nexit 9\n')
            binary.chmod(0o755)
            df = commands / 'df'
            df.write_text('#!/bin/sh\nprintf "Filesystem blocks Used Available Capacity Mounted\\nmock 1000000 0 1000000 0%% /\\n"\n')
            df.chmod(0o755)
            result = subprocess.run(['sh', str(WORKER), str(pkg), str(work)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('不兼容', result.stderr.decode())
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
            self.assertEqual((root / 'config.yaml').read_text(), 'legacy config remains private')
            self.assertFalse((root / '.update-transaction').exists())

    def test_full_update_converts_dns_template_and_preserves_configuration(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = pathlib.Path(tmp)
            root, pkg, work, commands = [base / n for n in ('root', 'pkg', 'work', 'commands')]
            for path in (root, pkg, work, commands): path.mkdir()
            cgi_bin = base / 'www/cgi-bin'
            cgi_bin.mkdir(parents=True)
            worker = base / 'update-worker.sh'
            fallback_page = base / 'tmp/var/wwwext/sockrocket/sockrocket.asp'
            # Keep the primary web path unusable. The installer can create
            # the fallback page even when its parent directory is absent.
            (base / 'www/ext').touch()
            worker.write_text(re.sub(r'/tmp/var/wwwext|/www', lambda match: str(base) + match[0], WORKER.read_text()))
            (root / 'scripts').mkdir()
            (root / 'scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (root / 'sockrocket-cli').write_text('old')
            (root / 'version').write_text('1.2.3')
            (root / 'api-token').write_text('fixture')
            endpoints = (root / 'sockrocket.cgi', cgi_bin / 'sockrocket.cgi')
            old_cgis = [bytes([x]) * (6 * 1024 * 1024) for x in (1, 2)]
            for endpoint, content in zip(endpoints, old_cgis):
                endpoint.write_bytes(content)
                endpoint.chmod(0o700)
            (root / 'config.yaml').write_text('original config')
            for name in ('bin', 'scripts', 'webui', 'res'): (pkg / name).mkdir()
            binary = pkg / 'bin/sockrocket-cli'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)
            control = WORKER.with_name('sockrocket.sh').read_text()
            (pkg / 'scripts/control.sh').write_text(control.replace('/jffs/addons/sockrocket',
                str(root)).replace('/www/cgi-bin', str(cgi_bin)))
            (pkg / 'scripts/sockrocket.sh').write_text('#!/bin/sh\ncase "$1" in api-restart) '
                'sh "$SOCKROCKET_DIR/scripts/control.sh" install-cgi;; esac\n')
            (pkg / 'scripts/dnsmasq.conf').write_text('server=127.0.0.1#5300\n')
            (pkg / 'version').write_text('1.2.3')
            (pkg / 'uninstall.sh').write_text('#!/bin/sh\n')
            (pkg / 'install.sh').write_text('#!/bin/sh\ntest -s "$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"\n'
                'if [ -f "$SOCKROCKET_DIR/fail-integration" ]; then\n'
                '  mkdir -p "' + str(fallback_page.parent) + '" && echo new > "' + str(fallback_page) + '"\n'
                '  cp "$SOCKROCKET_DIR/.update-transaction/external.tar.gz" "$SOCKROCKET_DIR/test-snapshot"\n'
                '  sh "$SOCKROCKET_DIR/scripts/control.sh" install-cgi || exit 8\n  exit 9\nfi\n')
            (work / 'worker.sh').write_text(worker.read_text())
            for name, body in {
                'df': 'printf "Filesystem blocks Used Available Capacity Mounted\\nmock 1000000 0 1000000 0%% /\\n"',
                'curl': "echo '{\"version\":\"1.2.3\"}'",
            }.items():
                tool = commands / name
                tool.write_text('#!/bin/sh\n' + body + '\n')
                tool.chmod(0o755)
            # Lose space after the durable backups are written. The running
            # installation must not be stopped or sent through rollback.
            df = commands / 'df'
            success_df = df.read_text()
            df.write_text('#!/bin/sh\nif test -f "$SOCKROCKET_DIR/.update-transaction/config-backed-up"; '
                          'then echo "mock 1000 999 1 99% /"; else echo "mock 1000000 0 1000000 0% /"; fi\n')
            (root / 'scripts/sockrocket.sh').write_text('#!/bin/sh\ntouch "$SOCKROCKET_DIR/service-called"\n')
            attempt = base / 'attempt'
            attempt.mkdir()
            failed = subprocess.run(['sh', str(worker), str(pkg), str(attempt)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True)
            self.assertNotEqual(failed.returncode, 0)
            message = (root / 'update-status').read_text()
            self.assertIn('备份后 空间不足', message)
            required, available, shortfall = map(int, re.findall(r'(\d+) KiB', message))
            self.assertEqual((available, shortfall), (1, required - 1))
            self.assertFalse((root / 'service-called').exists())
            self.assertFalse((root / '.update-transaction').exists())
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
            self.assertEqual((root / 'config.yaml').read_text(), 'original config')
            df.write_text(success_df)
            # Keep the legacy copies from consuming another 12 MiB on flash
            # during rollback. The temporary filesystem can stage them.
            tar = commands / 'tar'
            tar.write_text('#!/bin/sh\ncase "$2:$4" in */external.tar.gz:"$SOCKROCKET_DIR/.update-transaction") exit 28;; esac\n'
                           'exec "' + shutil.which('tar') + '" "$@"\n')
            tar.chmod(0o755)
            # Roll back after the candidate has already migrated both CGI copies.
            rollback = base / 'rollback'
            rollback.mkdir()
            (rollback / 'worker.sh').write_text(worker.read_text())
            fail_marker = root / 'fail-integration'
            fail_marker.touch()
            failed = subprocess.run(['sh', str(worker), str(pkg), str(rollback)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True, timeout=20)
            self.assertNotEqual(failed.returncode, 0)
            self.assertTrue((root / 'update-status').read_text().startswith('rolled-back:'), failed.stderr)
            for endpoint, content in zip(endpoints, old_cgis):
                self.assertFalse(endpoint.is_symlink())
                self.assertEqual(endpoint.read_bytes(), content)
            self.assertFalse(fallback_page.exists())
            # A read-only OEM tree cannot stage or remove a CGI entry, even
            # when the updater runs as root. The JFFS symlink remains restorable
            # without storing its target executable in the external archive.
            endpoints[0].unlink()
            endpoints[0].symlink_to(root / 'sockrocket-cli')
            endpoints[1].write_text('OEM endpoint')
            fallback_page.write_text('original page')
            for name in ('mkdir', 'rm'):
                tool = commands / name
                tool.write_text('#!/bin/sh\nfor arg do case "$arg" in "' + str(cgi_bin) +
                    '/"*) exit 1;; esac; done\nexec "' + shutil.which(name) + '" "$@"\n')
                tool.chmod(0o755)
            rollback.mkdir()
            (rollback / 'worker.sh').write_text(worker.read_text())
            failed = subprocess.run(['sh', str(worker), str(pkg), str(rollback)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True, timeout=20)
            self.assertNotEqual(failed.returncode, 0)
            self.assertTrue((root / 'update-status').read_text().startswith('rolled-back:'), failed.stderr)
            self.assertTrue(endpoints[0].is_symlink())
            self.assertEqual(endpoints[1].read_text(), 'OEM endpoint')
            self.assertEqual(fallback_page.read_text(), 'original page')
            with tarfile.open(root / 'test-snapshot') as archive:
                self.assertTrue(archive.getmember('external' + str(endpoints[0])).issym())
                self.assertNotIn('external' + str(endpoints[1]), archive.getnames())
            for name in ('mkdir', 'rm'):
                (commands / name).unlink()
            fail_marker.unlink()
            result = subprocess.run(['sh', str(worker), str(pkg), str(work)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root), 'SOCKROCKET_RUNTIME_DIR': str(root / 'runtime'),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'scripts/dnsmasq.conf.template').read_text(), 'server=127.0.0.1#5300\n')
            self.assertFalse((root / 'scripts/dnsmasq.conf').exists())
            for endpoint in endpoints:
                self.assertTrue(endpoint.is_symlink())
                self.assertTrue(endpoint.samefile(root / 'sockrocket-cli'))
            self.assertEqual((root / 'config.yaml').read_text(), 'original config')
            self.assertFalse((root / '.update-transaction').exists())

    def test_committed_transaction_only_cleans_backup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            tx.mkdir()
            (tx / 'committed').touch()
            (root / 'sockrocket-cli').write_text('new')
            self.assertEqual(self.run_recovery(root).returncode, 0)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'new')
            self.assertFalse(tx.exists())

    def test_failed_restart_retains_recovery_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            (tx / 'old/scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 1\n')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('0')
            self.assertNotEqual(self.run_recovery(root).returncode, 0)
            self.assertTrue(tx.exists())

if __name__ == '__main__':
    unittest.main()
