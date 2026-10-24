const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {webcrypto} = require('node:crypto');
const source = fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
function fixture() {
  const requests=[], timers=new Map(), elements={}, messages=[], copied=[]; let next=0;
  const element=id => elements[id] ||= {value:({'sub-name':'fixture','sub-url':'https://example.invalid/subscription','sub-format':'auto'})[id] || '',disabled:false};
  const context=vm.createContext({srElement:element, srApi:(...args)=>requests.push(args),
    window:{crypto:webcrypto}, Uint8Array, URL,
    srCopyText:(text)=>copied.push(text), srShowMsg:(message)=>messages.push(message), toastResp(){}, srLoadNodes(){}, refreshStatus(){},
    setTimeout(cb){const id=++next;timers.set(id,cb);return id;},clearTimeout:id=>timers.delete(id),
    SR_STATE:{},SR_PAGE_LOADED:{},srEsc:value=>value});
  vm.runInContext(source.slice(source.indexOf('function srLoadSubs()'),source.indexOf('/* ══ Routing rules')),context);
  const request=action=>{const index=requests.findIndex(r=>r[0]===action); assert.ok(index>=0,action);return requests.splice(index,1)[0];};
  const tick=()=>{const [id,cb]=timers.entries().next().value;timers.delete(id);cb();};
  return {context,requests,request,tick,element,messages,copied};
}
const f=fixture();f.context.addSub();
let start=f.request('subscription_start');
assert.equal(start[1].action,'add_sub');assert.equal(start[4],12000);
assert.equal(f.element('sub-add').disabled,true);
start[2]({ok:true,job:{id:'fixture-job',state:'running',stage:'fetching',total:4,completed:0}});
f.tick();let poll=f.request('subscription_job');assert.equal(poll[1].id,'fixture-job');
poll[3]();assert.equal(f.element('sub-update').disabled,true,'lost poll must not report terminal failure');
f.tick();poll=f.request('subscription_job');poll[2]({ok:true,job:{id:'fixture-job',state:'running',stage:'applying',total:4,completed:4}});
f.element('sub-url').value='https://example.invalid/new-input';
f.tick();f.request('subscription_job')[2]({ok:true,job:{id:'fixture-job',state:'finished',result:{ok:true}}});
assert.equal(f.element('sub-url').value,'https://example.invalid/new-input','new typed URL must survive old job completion');
assert.equal(f.element('sub-add').disabled,false);
const reload=fixture();reload.context.restoreSubscriptionJob();
reload.request('subscription_job')[2]({ok:true,job:{id:'resumed-job',state:'running',completed:2,total:4}});
assert.equal(reload.element('sub-add').disabled,true);
reload.tick();assert.equal(reload.request('subscription_job')[1].id,'resumed-job');
const lost=fixture();lost.context.updateSubs();
const launch=lost.request('subscription_start');const launchedId=launch[1].request_id;
assert.match(launchedId,/^[0-9a-f]{32}$/);
launch[3]({error:'Backend request failed; check service status'});
let recovered=lost.request('subscription_job');
assert.equal(recovered[1].id,launchedId,'lost launch queries its own ID, never an unrelated old job');
recovered[3]({error_code:'subscription_job_not_found'});
for(let index=0;index<3;index++) {
  lost.tick();lost.request('subscription_job')[3]({error_code:'subscription_job_not_found'});
}
assert.match(lost.messages.at(-1),/Backend request failed/,'original launch error must remain visible');
assert.equal(lost.element('sub-add').disabled,false);
const rejected=fixture();rejected.context.addSub();
rejected.request('subscription_start')[3]({error_code:'subscription_start_failed',msg:'Cannot start subscription job',stage:'save_request',cause:'PermissionDenied'});
assert.match(rejected.messages.at(-1),/save_request: PermissionDenied/);
assert.equal(rejected.requests.length,0,'confirmed launch rejection must not be hidden by an empty status query');
assert.equal(rejected.element('sub-url').value,'https://example.invalid/subscription');
const delayed=fixture();delayed.context.addSub();
const delayedLaunch=delayed.request('subscription_start');const delayedId=delayedLaunch[1].request_id;
delayedLaunch[3]({error_code:'timeout'});
delayed.request('subscription_job')[3]({error_code:'subscription_job_not_found'});
delayed.tick();delayed.request('subscription_job')[2]({ok:true,job:{id:delayedId,state:'running',stage:'fetching'}});
assert.equal(delayed.element('sub-add').disabled,true,'late launch must recover the same job');
delayed.tick();delayed.request('subscription_job')[2]({ok:true,job:{id:delayedId,state:'finished',result:{ok:true}}});
assert.equal(delayed.element('sub-url').value,'','only confirmed matching job may clear the submitted input');
console.log('PASS: exact job recovery, original errors, delayed launch, reload and input preservation');

const diagnostic=fixture();
diagnostic.context.showSubscriptionDiagnostic({error_code:'subscription_start_failed',stage:'save_request',cause:'PermissionDenied',os_error:13,msg:'private https://example.invalid/secret-token /private/path',hint:'private secret-token'},'a'.repeat(32));
assert.equal(diagnostic.element('sub-diagnostic').hidden,false);
diagnostic.context.copySubscriptionDiagnostic();
assert.match(diagnostic.copied[0],/权限不足/);assert.match(diagnostic.copied[0],/系统错误码：13/);
assert.doesNotMatch(diagnostic.copied[0],/secret-token|example.invalid|private.path/);
console.log('PASS: persistent diagnostic, actionable reason, error code and private-field exclusion');

const oldDuplicate=fixture();oldDuplicate.context.showSubscriptionDiagnostic({ok:false,msg:'This subscription already exists; update the existing entry'},'a'.repeat(32));
oldDuplicate.context.copySubscriptionDiagnostic();
assert.match(oldDuplicate.copied[0],/subscription_exists/);
assert.match(oldDuplicate.copied[0],/校验订阅/);
assert.match(oldDuplicate.copied[0],/全部更新/);
const oldParse=fixture();oldParse.context.showSubscriptionDiagnostic({ok:false,msg:'Subscription saved, but fetch/parse failed: No valid proxy nodes found in subscription'},'a'.repeat(32));
oldParse.context.copySubscriptionDiagnostic();assert.match(oldParse.copied[0],/subscription_no_valid_nodes/);
assert.match(oldParse.copied[0],/解析订阅节点/);
console.log('PASS: historical duplicate/empty imports retain actionable reasons without private messages');

const optional=fixture();optional.element('sub-name').value='';optional.context.addSub();
assert.equal(optional.request('subscription_start')[1].body.name,'');
const badUrl=fixture();badUrl.element('sub-url').value='https://';badUrl.context.addSub();
assert.equal(badUrl.requests.length,0,'invalid URLs must not launch or save a subscription');
const notes=fixture();notes.context.renderSubscriptionNotes({ok:true,compatibility_notes:['hysteria2_browser_fingerprint_hint','https://example.invalid/private-token']});
assert.equal(notes.element('sub-notes').hidden,false);
assert.match(notes.element('sub-notes-text').textContent,/不用于模拟/);
assert.doesNotMatch(notes.element('sub-notes-text').textContent,/private-token/);
notes.context.renderSubscriptionNotes({ok:true,compatibility_notes:['hysteria2_chrome_quic_profile']});
assert.equal(notes.element('sub-notes').hidden,false);
assert.match(notes.element('sub-notes-text').textContent,/已启用 Chrome 115/);
assert.match(notes.element('sub-notes-text').textContent,/不模拟 ALPS/);
notes.context.renderSubscriptionNotes({ok:true,compatibility_notes:['hysteria2_mport_fixed_port']});
assert.equal(notes.element('sub-notes').hidden,false);
assert.match(notes.element('sub-notes-text').textContent,/固定端口导入/);
assert.match(notes.element('sub-notes-text').textContent,/不会定期切换端口/);
notes.context.renderSubscriptionNotes({ok:true,compatibility_notes:[]});
assert.equal(notes.element('sub-notes').hidden,true);
console.log('PASS: optional names, invalid-address rejection and informational compatibility notes');
