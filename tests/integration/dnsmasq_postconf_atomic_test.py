#!/usr/bin/env python3
"""Force the firmware postconf hook and control-script repair to overlap."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
helper = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / 'merlin/scripts/dnsmasq-postconf.sh'
with tempfile.TemporaryDirectory() as directory:
    work = Path(directory)
    config = work / 'dnsmasq.conf'
    config.write_text('pid-file=/var/run/dnsmasq.pid\nservers-file=/tmp/resolv.dnsmasq\ndhcp-range=192.0.2.10,192.0.2.20,12h\n')
    upstream = work / 'upstream'
    upstream.write_text('server=127.0.0.1#5300\n')
    stub = work / 'stub'
    stub.mkdir()
    # Both helpers validate their complete output before either commits it.
    # The second helper proceeds only after the first has removed/renamed its
    # temporary file. A shared temporary pathname plus cat > config loses it.
    grep = stub / 'grep'
    grep.write_text('#!' + sys.executable + '\n' + r'''
import os, pathlib, subprocess, sys, time
args = sys.argv[1:]
real = os.environ['POSTCONF_REAL_GREP']
if '-Fx' not in args:
    os.execv(real, [real] + args)
result = subprocess.run([real] + args)
if result.returncode:
    sys.exit(result.returncode)
root = pathlib.Path(os.environ['POSTCONF_BARRIER'])
try:
    (root / 'leader').mkdir()
    first = True
    (root / 'leader-path').write_text(args[-1])
except FileExistsError:
    first = False
(root / ('ready-' + str(os.getpid()))).touch()
end = time.monotonic() + 5
while len(list(root.glob('ready-*'))) < 2:
    if time.monotonic() > end: sys.exit(90)
    time.sleep(.001)
if not first:
    path = pathlib.Path((root / 'leader-path').read_text())
    while path.exists():
        if time.monotonic() > end: sys.exit(91)
        time.sleep(.001)
sys.exit(0)
''')
    grep.chmod(0o755)
    environment = dict(os.environ, PATH=str(stub) + os.pathsep + os.environ['PATH'],
                       SOCKROCKET_DNS_MAIN_CONFIG=str(config),
                       SOCKROCKET_DNS_UPSTREAM_FILE=str(upstream),
                       POSTCONF_REAL_GREP=shutil.which('grep'), POSTCONF_BARRIER=str(work))
    processes = [subprocess.Popen(['sh', str(helper), str(config)], env=environment,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE) for _ in range(2)]
    outputs = [p.communicate(timeout=10) for p in processes]
    statuses = [p.returncode for p in processes]
    text = config.read_text()
    assert statuses == [0, 0], f'concurrent helpers failed: {statuses}; config bytes={len(text)}; errors={outputs}'
    assert text and 'no-resolv\n' in text and 'server=127.0.0.1#5300\n' in text
    assert 'dhcp-range=192.0.2.10,192.0.2.20,12h\n' in text
    assert 'servers-file=' not in text
    assert not list(work.glob('dnsmasq.conf.sockrocket-*')), 'temporary state was not cleaned up'
    print('PASS: overlapping firmware/control helpers preserve complete DNS configuration')
