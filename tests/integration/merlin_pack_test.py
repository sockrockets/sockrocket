"""Validate per-platform archives without a Rust build or router access."""
import hashlib
import os
import re
from pathlib import Path
import struct
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
                header = bytearray(64)
                header[:7] = b'\x7fELF' + bytes([2 if arch == 'aarch64' else 1, 1, 1])
                struct.pack_into('<HH', header, 16, 2, 183 if arch == 'aarch64' else 40)
                if arch == 'aarch64':
                    struct.pack_into('<Q', header, 32, 64)
                    struct.pack_into('<HH', header, 54, 56, 1)
                    ph_size = 56
                else:
                    struct.pack_into('<I', header, 28, 64)
                    struct.pack_into('<HH', header, 42, 32, 1)
                    ph_size = 32
                if arch != 'aarch64':
                    flags = 0x05000400 if arch == 'armv7' else 0x05000200
                    header[36:40] = flags.to_bytes(4, 'little')
                binary.write_bytes(header + (1).to_bytes(4, 'little') + bytes(ph_size - 4))
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
            guide = (ROOT / 'docs/merlin-models.md').read_bytes()
            self.assertEqual((out / 'merlin-models.md').read_bytes(), guide)
            rows = re.findall(r'^\| ([^|]+) \| [^|]+ \| `(\w+)` \| `(sockrocket-merlin-(\w+)\.tar\.gz)` \|$',
                              guide.decode(), re.MULTILINE)
            self.assertEqual({name for _, _, name, _ in rows}, {p.name for p in out.glob('*.tar.gz')})
            # Representative firmware families, including similar AC/AX model names.
            reference_models = {'arm': 'RT-AC88U', 'hnd': 'RT-AX58U', 'hnd_v8': 'RT-AX88U',
                                'qca': 'RT-AX89X', 'mtk': 'TX-AX6000',
                                'ipq32': 'ZenWiFi BD4', 'ipq64': 'TUF-BE6500'}
            known_models = set()
            for models, _, _, platform in rows:
                names = models.split('、')
                self.assertIn(reference_models[platform], names)
                self.assertTrue(known_models.isdisjoint(names))
                known_models.update(names)
            markers = {platform: marker for _, marker, _, platform in rows}
            homepage = (ROOT / 'docs/site/index.html').read_text()
            install_guide = (ROOT / 'docs/site/docs/merlin.html').read_text()
            for models, _, name, platform in rows:
                # Check the actual download link in each rendered table row.
                expected_models = models.replace('、', ', ')
                for page in (homepage, install_guide):
                    page_rows = re.findall(r'<tr><td><code>' + platform + r'</code>.*?</tr>', page)
                    self.assertTrue(page_rows, platform)
                    for row in page_rows:
                        self.assertIn('>' + expected_models + '</td>', row)
                        self.assertIn('/download/' + name + '"', row)
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
                    self.assertEqual(archive.extractfile('sockrocket/merlin-models.md').read(), guide)
                    self.assertEqual(markers[platform], 'arm386' if platform == 'arm' else platform)
                    binary = files['sockrocket/bin/sockrocket-cli']
                    self.assertTrue(binary.mode & 0o111)
                    self.assertEqual(archive.extractfile(binary).read(), binaries[arch].read_bytes())
                    self.assertEqual(archive.extractfile('sockrocket/.valid').read(),
                                     b'arm386\n' if platform == 'arm' else (platform + '\n').encode())
                    for relative in ('install.sh', 'uninstall.sh', 'config.yaml.template',
                                     'version', 'scripts/update.sh', 'webui/sockrocket.asp'):
                        self.assertEqual(archive.extractfile('sockrocket/' + relative).read(),
                                         (ROOT / 'merlin' / relative).read_bytes())
            dynamic = work / 'dynamic'
            payload = bytearray(binaries['armv7sf'].read_bytes())
            payload[64:68] = (3).to_bytes(4, 'little')  # PT_INTERP
            dynamic.write_bytes(payload)
            dynamic.chmod(0o755)
            for bad_softfloat in (binaries['armv7'], binaries['aarch64'], dynamic, work / 'missing'):
                bad_out = work / 'invalid'
                failed = subprocess.run(
                    ['bash', str(ROOT / 'merlin/pack.sh')],
                    env={**os.environ, 'OUT_DIR': str(bad_out), 'AARCH64_BIN': str(binaries['aarch64']),
                         'ARMV7HF_BIN': str(binaries['armv7']), 'ARMV7SF_BIN': str(bad_softfloat)},
                    capture_output=True, timeout=30,
                )
                self.assertNotEqual(failed.returncode, 0)
                self.assertFalse(list(bad_out.glob('*.tar.gz')))
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
