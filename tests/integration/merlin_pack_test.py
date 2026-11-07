"""Validate per-platform archives without a Rust build or router access."""
import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PackageTest(unittest.TestCase):
    def test_platform_payloads_and_update_compatible_archives(self):
        with tempfile.TemporaryDirectory(prefix='sockrocket-pack-') as directory:
            work = Path(directory)
            binaries = {}
            for arch in ('aarch64', 'armv7', 'armv7sf'):
                binary = work / arch
                binary.write_text(f'#!/bin/sh\necho "sockrocket-cli fixture-{arch}"\n')
                binary.chmod(0o755)
                binaries[arch] = binary
            out = work / 'packages'
            result = subprocess.run(
                ['bash', str(ROOT / 'merlin/pack.sh')],
                env={**os.environ, 'OUT_DIR': str(out),
                     'AARCH64_BIN': str(binaries['aarch64']),
                     'ARMV7HF_BIN': str(binaries['armv7']),
                     'ARMV7SF_BIN': str(binaries['armv7sf'])},
                capture_output=True, text=True, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            platforms = {'hnd_v8': 'aarch64', 'mtk': 'aarch64', 'ipq64': 'aarch64',
                         'hnd': 'armv7', 'qca': 'armv7', 'ipq32': 'armv7', 'arm': 'armv7sf'}
            self.assertEqual(len(list(out.glob('*.tar.gz'))), len(platforms))
            for platform, arch in platforms.items():
                with self.subTest(platform=platform), tarfile.open(
                    out / f'sockrocket-merlin-{platform}.tar.gz'
                ) as archive:
                    members = archive.getmembers()
                    # The online updater rejects links and special files.
                    self.assertTrue(all(m.isfile() or m.isdir() for m in members))
                    files = {m.name: m for m in members if m.isfile()}
                    self.assertEqual(
                        [n for n in files if n.startswith('sockrocket/bin/')],
                        ['sockrocket/bin/sockrocket-cli'],
                    )
                    # Software Center discovers the plugin page before install.sh runs.
                    self.assertIn('sockrocket/webs/Module_sockrocket.asp', files)
                    self.assertEqual(
                        archive.extractfile('sockrocket/webs/Module_sockrocket.asp').read(),
                        archive.extractfile('sockrocket/webui/sockrocket.asp').read(),
                    )
                    self.assertFalse(any('/._' in n for n in files))
                    binary = files['sockrocket/bin/sockrocket-cli']
                    self.assertTrue(binary.mode & 0o111)
                    self.assertEqual(archive.extractfile(binary).read(), binaries[arch].read_bytes())
                    self.assertEqual(archive.extractfile('sockrocket/.valid').read(),
                                     (platform + '\n').encode())
                    for relative in ('install.sh', 'uninstall.sh', 'config.yaml.template',
                                     'version', 'scripts/update.sh', 'webui/sockrocket.asp'):
                        self.assertEqual(archive.extractfile('sockrocket/' + relative).read(),
                                         (ROOT / 'merlin' / relative).read_bytes())
            for arch, binary in binaries.items():
                self.assertEqual((out / f'sockrocket-cli-linux-{arch}').read_bytes(),
                                 binary.read_bytes())
            checksums = {}
            for line in (out / 'SHA256SUMS-merlin.txt').read_text().splitlines():
                digest, name = line.split()
                checksums[name] = digest
                self.assertEqual(hashlib.sha256((out / name).read_bytes()).hexdigest(), digest)
            self.assertEqual(set(checksums), {p.name for p in out.iterdir()
                                            if p.name != 'SHA256SUMS-merlin.txt'})


if __name__ == '__main__':
    unittest.main()
