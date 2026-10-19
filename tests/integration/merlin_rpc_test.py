"""Local-only firmware bridge regression; no credentials or live networking."""
from pathlib import Path
import tempfile,os,subprocess,json,stat
with tempfile.TemporaryDirectory(prefix='sockrocket-rpc-') as directory:
    root=Path(directory);(root/'bin').mkdir();(root/'dbus').mkdir();(root/'bodies').mkdir()
    def executable(path,text):path.write_text(text);path.chmod(0o700)
    (root/'base.sh').write_text('ID=$1\n')
    executable(root/'bin/dbus','#!/bin/sh\ncase "$1" in get) cat "$FIXTURE/dbus/$2" 2>/dev/null;; remove) rm -f "$FIXTURE/dbus/$2";; esac\n')
    executable(root/'bin/curl','#!/bin/sh\nexit 0\n')
    executable(root/'bin/backend','''#!/bin/sh
cat > "$FIXTURE/bodies/${QUERY_STRING#action=}"
printf 'Content-Type: application/json\\nCache-Control: no-cache\\n\\n{"ok":true}\\n'
''')
    bridge=root/'bridge.sh';bridge.write_text(Path('merlin/scripts/sockrocket_api.sh').read_text().replace('. /koolshare/scripts/base.sh','. "'+str(root/'base.sh')+'"'))
    env=dict(os.environ,FIXTURE=str(root),PATH=str(root/'bin')+':'+os.environ['PATH'],SOCKROCKET_BIN=str(root/'bin/backend'),SOCKROCKET_RPC_UPLOAD_DIR=str(root/'upload'),SOCKROCKET_RPC_RUNTIME_DIR=str(root/'runtime'))
    def seed(seq,value): (root/'dbus'/('sockrocket_rpc_body_'+seq)).write_text(json.dumps(value))
    def run(action,seq):return subprocess.run(['sh',str(bridge),'123456',action,seq],env=env,check=True)
    a='a'*32;b='b'*32
    seed(a,{'value':'A'});seed(b,{'value':'B'})
    jobs=[subprocess.Popen(['sh',str(bridge),'123456',action,seq],env=env) for action,seq in [('version',a),('status',b)]]
    assert all(p.wait()==0 for p in jobs)
    assert json.loads((root/'bodies/version').read_text())=={'value':'A'}
    assert json.loads((root/'bodies/status').read_text())=={'value':'B'}
    assert not list((root/'dbus').iterdir())
    result=root/'upload'/('sockrocket_rpc_'+a+'.json')
    assert stat.S_IMODE(result.stat().st_mode)==0o600
    assert json.loads(result.read_text())['ok']
    run('rpc_ack',a);assert not result.exists()
    for n in range(1,5):
        slot=root/'runtime'/('slot-'+str(n));slot.mkdir();(slot/'pid').write_text(str(os.getpid()))
    c='c'*32;seed(c,{})
    run('version',c)
    assert json.loads((root/'upload'/('sockrocket_rpc_'+c+'.json')).read_text())['error_code']=='busy'
    d='d'*32;seed(d,{})
    run('stop',d)
    assert json.loads((root/'upload'/('sockrocket_rpc_'+d+'.json')).read_text())['ok']
    executable(root/'bin/backend','#!/bin/sh\nexit 7\n')
    run('stop',d)
    assert 'error' in json.loads((root/'upload'/('sockrocket_rpc_'+d+'.json')).read_text())
print('PASS: concurrent request isolation, body removal, private results, ACK deletion, bounded jobs, priority Stop and backend errors')
