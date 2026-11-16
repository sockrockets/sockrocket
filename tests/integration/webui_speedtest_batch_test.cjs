const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = fs.readFileSync('merlin/webui/sockrocket.asp', 'utf8');
const code = source.slice(source.indexOf('function newRequestId('), source.indexOf('function apiKscRequest(')) +
  source.slice(source.indexOf('var speedTestPollPending ='), source.indexOf('function clearAddNodeSecrets'));
const timers = [], requests = [], button = {};
let renders = 0;
const state = {nodes: [{test_id:'0-abc'}, {test_id:'1-abc'}], lat:{}};
const context = vm.createContext({SR_STATE:state, Uint8Array,
 window:{crypto:require('node:crypto').webcrypto}, nodeViewKey:n=>n.test_id,
 srElement:()=>button, saveSpeedTests(){}, renderNodes(){renders++;}, renderChrome(){},
 srShowMsg(){}, setTimeout:cb=>{timers.push(cb);return timers.length;}, clearTimeout(){},
 srApi:(action,data,ok,fail)=>requests.push({action,data,ok,fail})});
vm.runInContext(code+';testAllNodes();',context);
assert.equal(requests[0].action,'speedtest_start');
assert.match(requests[0].data.request_id,/^[0-9a-f]{32}$/);
requests.shift().ok({ok:true,job:{state:'running',completed:0,total:2,results:[]}});
assert.equal(requests[0].action,'speedtest_job');
const partial={state:'running',completed:1,total:2,results:[{key:'0-abc',latency:10}]};
requests.shift().ok({ok:true,job:partial});
assert.equal(state.lat[0],10);
assert.equal(state.testingAll,true);
const finished={state:'finished',completed:2,total:2,results:[{key:'0-abc',latency:10},{key:'1-abc',latency:-1}]};
timers.shift()();requests.shift().ok({ok:true,job:finished});
assert.equal(state.lat[1],-1);assert.equal(state.testingAll,false);
// A fresh page attaches by polling, never relaunches the batch.
vm.runInContext(code+';pollSpeedTestJob();',context);
assert.equal(requests[0].action,'speedtest_job');
requests.shift().ok({ok:true,job:finished});
assert.equal(state.lat[0],10);
assert.ok(renders<=4);
console.log('PASS: one backend batch, duplicate identities, failures, bounded rendering, reload reattachment');

context.confirm = () => true;
vm.runInContext(source.slice(source.indexOf('var nodeSwitchPending'), source.indexOf('function testNode(')) +
  source.slice(source.indexOf('function delNode('), source.indexOf('/* ══ Subscriptions')), context);
context.setNode(1);
context.delNode(0);
assert.equal(requests.shift().data.expected_id, state.nodes[1].test_id);
assert.equal(requests.shift().data.expected_id, state.nodes[0].test_id);
