const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
function fixture() {
  const requests=[], timers=new Map(), elements={}; let next=0;
  const element=id => elements[id] ||= {value:({'sub-name':'fixture','sub-url':'https://example.invalid/subscription','sub-format':'auto'})[id] || '',disabled:false};
  const context=vm.createContext({srElement:element, srApi:(...args)=>requests.push(args),
    srShowMsg(){}, toastResp(){}, srLoadNodes(){}, refreshStatus(){},
    setTimeout(cb){const id=++next;timers.set(id,cb);return id;},clearTimeout:id=>timers.delete(id),
    SR_STATE:{},SR_PAGE_LOADED:{},srEsc:value=>value});
  vm.runInContext(source.slice(source.indexOf('function srLoadSubs()'),source.indexOf('/* ══ Routing rules')),context);
  const request=action=>{const index=requests.findIndex(r=>r[0]===action); assert.ok(index>=0,action);return requests.splice(index,1)[0];};
  const tick=()=>{const [id,cb]=timers.entries().next().value;timers.delete(id);cb();};
  return {context,requests,request,tick,element};
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
const lost=fixture();lost.context.updateSubs();lost.request('subscription_start')[3]();
assert.equal(lost.request('subscription_job')[1].id,undefined,'lost launch recovers backend active job');
console.log('PASS: background launch, polling recovery, page reload and input preservation');
