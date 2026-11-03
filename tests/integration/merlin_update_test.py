"""Exercise durable recovery without a router or root filesystem changes."""
import os
import pathlib
import subprocess
import tempfile
import tarfile
import unittest

WORKER = pathlib.Path(__file__).resolve().parents[2] / 'merlin/scripts/update.sh'

class RecoveryTest(unittest.TestCase):
    def run_recovery(self, root, unhealthy=False):
        tools = root / '.test-tools'
        tools.mkdir(exist_ok=True)
        for name, body in {'curl': "echo '{\"version\":\"test-version\"}'", 'nslookup': 'exit 0'}.items():
            path = tools / name
            path.write_text('#!/bin/sh\n' + ('exit 7' if unhealthy else body) + '\n')
            path.chmod(0o755)
        if not (root / 'config.yaml').exists(): (root / 'config.yaml').write_text('socks_port: 1080\n')
        (root / 'api-token').write_text('test-token')
        (root / 'sockrocket.pid').write_text(str(os.getpid()))
        return subprocess.run(['sh', str(WORKER), '--recover'],
            env={**os.environ, 'SOCKROCKET_DIR': str(root),
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
            (root / '.update-restore-lock').mkdir()
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
            self.assertFalse((root / '.update-restore-lock').exists())

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
            df = commands / 'df'
            df.write_text('#!/bin/sh\nprintf "Filesystem 1024-blocks Used Available Capacity Mounted\\nmock 100 99 1 99%% /\\n"\n')
            df.chmod(0o755)
            result = subprocess.run(['sh', str(WORKER), str(pkg), str(work), '--check-space'],
                env={**os.environ, 'SOCKROCKET_DIR': str(root),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('空间不足', result.stderr.decode())
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
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
                env={**os.environ, 'SOCKROCKET_DIR': str(root),
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
            (root / 'scripts').mkdir()
            (root / 'scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (root / 'sockrocket-cli').write_text('old')
            (root / 'config.yaml').write_text('original config')
            for name in ('bin', 'scripts', 'webui', 'res'): (pkg / name).mkdir()
            binary = pkg / 'bin/sockrocket-cli'
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)
            (pkg / 'scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (pkg / 'scripts/dnsmasq.conf').write_text('server=127.0.0.1#5300\n')
            (pkg / 'version').write_text('1.2.3')
            (pkg / 'uninstall.sh').write_text('#!/bin/sh\n')
            (pkg / 'install.sh').write_text('#!/bin/sh\ntest -s "$SOCKROCKET_DIR/scripts/dnsmasq.conf.template"\n')
            (work / 'worker.sh').write_text(WORKER.read_text())
            for name, body in {
                'df': 'printf "Filesystem blocks Used Available Capacity Mounted\\nmock 1000000 0 1000000 0%% /\\n"',
                'curl': "echo '{\"version\":\"1.2.3\"}'",
            }.items():
                tool = commands / name
                tool.write_text('#!/bin/sh\n' + body + '\n')
                tool.chmod(0o755)
            result = subprocess.run(['sh', str(WORKER), str(pkg), str(work)],
                env={**os.environ, 'SOCKROCKET_DIR': str(root),
                     'PATH': str(commands) + os.pathsep + os.environ['PATH']}, capture_output=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'scripts/dnsmasq.conf.template').read_text(), 'server=127.0.0.1#5300\n')
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
