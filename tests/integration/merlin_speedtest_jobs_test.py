"""Exercise the real detached batch worker against local TCP fixtures."""
import os, pathlib, subprocess, tempfile, json, socket, threading, time
binary = str(pathlib.Path('target/debug/sockrocket-cli').resolve())
with tempfile.TemporaryDirectory(prefix='sockrocket-speedtest-fixture-') as tmp:
    root = pathlib.Path(tmp)
    listener = socket.socket(); listener.bind(('127.0.0.1',0)); listener.listen(128)
    port=listener.getsockname()[1]
    accepted=[]
    def accept():
        while True:
            try:
                connection,_=listener.accept(); accepted.append(1); connection.close()
            except OSError: return
    threading.Thread(target=accept,daemon=True).start()
    nodes=[{'name':'duplicate','server':'127.0.0.1','port':port,'protocol':{'type':'shadowsocks','cipher':'aes-128-gcm','password':'fixture'}} for _ in range(128)]
    (root/'config.yaml').write_text(json.dumps({'nodes':nodes}))
    env=dict(os.environ,SOCKROCKET_DIR=tmp,SOCKROCKET_LOCAL_RPC='1',REQUEST_METHOD='POST',CONTENT_LENGTH='')
    def api(action,body={}):
        result=subprocess.run([binary,'cgi'],input=json.dumps(body),env=dict(env,QUERY_STRING='action='+action),text=True,capture_output=True,check=True)
        return json.loads(result.stdout.split('\n\n',1)[1])
    details=api('node_details')['nodes']; assert len({n['test_id'] for n in details})==128
    job=api('speedtest_start',{'mode':'tcp','request_id':'fixture-1'})['job']
    deadline=time.monotonic()+25
    while time.monotonic()<deadline:
        # Each poll is a separate CGI process, just like page refreshes.
        state=api('speedtest_job')['job']
        if state['state']!='running': break
        time.sleep(.1)
    assert state['state']=='finished',state
    assert state['completed']==128 and len(state['results'])==128
    assert {r['key'] for r in state['results']}=={n['test_id'] for n in details}
    assert all(r['latency']>=0 for r in state['results']),state['results'][:2]
    assert len(accepted)==128, len(accepted)
    replay=api('speedtest_start',{'mode':'tcp','request_id':'fixture-1'})['job']
    assert replay['id']==job['id'] and replay['state']=='finished'
    assert api('speedtest_job')['job']['results']==state['results']
    # A bad node must finish as a failure without stopping its healthy peers.
    bad=dict(nodes[0], protocol={'type':'not-a-protocol'})
    (root/'config.yaml').write_text(json.dumps({'nodes':[bad,nodes[0]]}))
    mixed=api('speedtest_start',{'mode':'tcp','request_id':'fixture-2'})['job']
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        mixed=api('speedtest_job')['job']
        if mixed['state']!='running': break
        time.sleep(.1)
    assert mixed['state']=='finished' and mixed['completed']==2
    by_key={r['key']:r['latency'] for r in mixed['results']}
    details=api('node_details')['nodes']
    assert by_key[details[0]['test_id']]==-1
    assert by_key[details[1]['test_id']]>=0
    listener.close()
print('PASS: 128 duplicate nodes all tested, detached lifetime, cached results, idempotent launch')
