"""Exercise durable recovery without a router or root filesystem changes."""
import os
import pathlib
import subprocess
import tempfile
import unittest

WORKER = pathlib.Path(__file__).resolve().parents[2] / 'merlin/scripts/update.sh'

class RecoveryTest(unittest.TestCase):
    def run_recovery(self, root):
        return subprocess.run(['sh', str(WORKER), '--recover'],
                              env={**os.environ, 'SOCKROCKET_DIR': str(root)}, capture_output=True)

    def test_interrupted_publish_restores_by_rename(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            tx = root / '.update-transaction'
            (tx / 'old/scripts').mkdir(parents=True)
            (tx / 'old/scripts/sockrocket.sh').write_text('#!/bin/sh\nexit 0\n')
            (tx / 'old/sockrocket-cli').write_text('old')
            (tx / 'prepared').touch()
            (tx / 'running').write_text('0')
            (root / 'sockrocket-cli').write_text('new')
            result = self.run_recovery(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'sockrocket-cli').read_text(), 'old')
            self.assertFalse(tx.exists())
            self.assertEqual(self.run_recovery(root).returncode, 0)

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
