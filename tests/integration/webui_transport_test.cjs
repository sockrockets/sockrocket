const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const {webcrypto} = require('node:crypto');
const source = fs.readFileSync('merlin/webui/sockrocket.asp', 'utf8');
const requests = [];
class XHR {
  open(method, url) { this.method = method; this.url = url; }
  setRequestHeader() {}
  send(body) { this.body = body; requests.push(this); }
  reply(status, body) { this.status = status; this.responseText = body; this.onload(); }
}
const context = vm.createContext({XMLHttpRequest: XHR, Math, Date, JSON, Uint8Array,
  window: {crypto: webcrypto}, TextEncoder, btoa, location: {hostname: 'router'}, srShowMsg() {},
  setTimeout() { throw Error('unexpected retry'); }});
vm.runInContext(source.slice(source.indexOf('var TRANSPORT ='), source.indexOf('function b64utf8')) + source.slice(source.indexOf('function utf8b64'),source.indexOf('function srEsc')), context);
let first = 0, second = 0, stopped = 0;
context.apiKsc('version', null, () => first++);
context.apiKsc('status', null, () => second++);
assert.equal(requests.length, 1);
const job = JSON.parse(requests[0].body);
assert.ok(Number.isInteger(job.id) && job.id > 0 && job.id < 1000000);
assert.match(job.params[1], /^[0-9a-f]{32}$/);
assert.equal(Object.keys(job.fields)[0], 'sockrocket_rpc_b64_' + job.params[1]);
assert.equal(Buffer.from(job.fields[Object.keys(job.fields)[0]], 'base64').toString(), '{}');
context.apiKsc('stop', {}, () => stopped++);
assert.equal(JSON.parse(requests[1].body).params[0], 'stop', 'stop bypasses long queue');
assert.notEqual(JSON.parse(requests[1].body).params[1], job.params[1]);
requests[0].reply(200, JSON.stringify({result: job.id}));
assert.ok(requests[2].url.includes(job.params[1]));
requests[2].reply(200, '{"version":"test"}');
assert.equal(first, 1);
const cleanup = JSON.parse(requests[3].body);
assert.deepEqual(cleanup.params, ['rpc_ack', job.params[1]]);
assert.deepEqual(cleanup.fields, {});
assert.equal(JSON.parse(requests[4].body).params[0], 'status');
requests[4].reply(200, JSON.stringify({result: JSON.parse(requests[4].body).id}));
requests[5].reply(200, '{"running":false}');
assert.equal(second, 1);
let timeout = 0;
context.apiKsc('add_sub', {}, () => assert.fail(), () => timeout++, 35000);
const add = requests.at(-1);
assert.equal(add.timeout, 35000);
add.ontimeout();
assert.equal(timeout, 1);
context.apiKsc('version', null, () => {});
assert.equal(JSON.parse(requests.at(-1).body).params[0], 'version');
let bootCalls = [];
const boot = source.slice(source.lastIndexOf('probeTransport(function () {'), source.lastIndexOf('</script>'));
vm.runInNewContext(boot, {probeTransport: cb => cb(), srLoadNodes: () => bootCalls.push('nodes'),
  refreshStatus: () => bootCalls.push('status'), srPollUpdate: () => bootCalls.push('update-status'), setInterval() {},
  api: action => assert.fail('page boot must not invoke lifecycle: ' + action)});
assert.deepEqual(bootCalls, ['nodes', 'status', 'update-status']);
console.log('PASS: private request nonce/key, priority Stop, result cleanup, timeout and passive boot');

const payload={name:'示例 " \\ 节点',url:'https://example.invalid/sub?a=1&b=2'};
context.apiKscRequest('version',payload,()=>{});
const encoded=JSON.parse(requests.at(-1).body).fields;
const stored=Object.values(encoded)[0].replace(/\\/g,'\\\\').replace(/"/g,'\\"');
assert.deepEqual(JSON.parse(Buffer.from(stored,'base64').toString('utf8')),payload);
console.log('PASS: UTF-8 JSON survives Softcenter field escaping');

const preview={text:'DOMAIN,edited.invalid,DIRECT\n'.repeat(185),group:'测试'};
context.apiKscRequest('batch_add_rules',preview,()=>{});
let staged='',packet=JSON.parse(requests.at(-1).body);
const transfer=packet.params[1];
while(packet.params[0]==='rpc_rule_part'){
  assert.deepEqual(packet.params,['rpc_rule_part',transfer,String(staged.length)]);
  const part=packet.fields['sockrocket_rpc_b64_'+transfer];
  assert.ok(part.length>0 && part.length<=2048);
  staged+=part;
  requests.at(-1).reply(200,JSON.stringify({result:packet.id}));
  packet=JSON.parse(requests.at(-1).body);
}
assert.deepEqual(JSON.parse(Buffer.from(staged,'base64').toString()),preview);
assert.deepEqual(packet.params,['batch_add_rules',transfer,String(staged.length)]);
assert.deepEqual(packet.fields,{});
console.log('PASS: edited URL preview is staged in order and committed once');

const configElements = {};
const configRequests = [];
let beforeUnload;
const config = vm.createContext({
  srElement: id => configElements[id] ||= { value: '' },
  srApi: (...args) => configRequests.push(args),
  b64utf8: value => Buffer.from(value, 'base64').toString(),
  utf8b64: value => Buffer.from(value).toString('base64'),
  window: { addEventListener: (_, callback) => { beforeUnload = callback; } },
  ruleWorkspaceDirty: () => false, dnsPolicyDirty: false, SR_PAGE_LOADED: {},
  srShowMsg() {}, toastResp() {}, setTimeout() {}, refreshStatus() {}
});
vm.runInContext(source.slice(source.indexOf('var savedConfigText'), source.indexOf('var pendingUpdateAsset')), config);
config.srLoadConfig();
configRequests.shift()[2]({ok: false, msg: '读取失败'});
assert.equal(configElements['config-state'].textContent, '读取失败');
config.srLoadConfig();
configRequests.shift()[2]({config_base64: Buffer.from('first').toString('base64'), revision: 'r1'});
config.srSaveConfig();
assert.equal(configRequests.length, 0, 'unchanged save cannot restart a service');
const editor = configElements['config-text'];
editor.value = 'draft';
config.configEdited();
config.srSaveConfig();
const staleSave = configRequests.shift();
assert.equal(staleSave[1].expected_revision, 'r1');
staleSave[2]({ok: false, msg: '配置已被其他操作更新'});
assert.equal(editor.value, 'draft');
assert.equal(config.savedConfigText, 'first');
config.srSaveConfig();
configRequests.shift()[2]({ok: false, saved: true, revision: 'r2', msg: '服务重启失败'});
assert.equal(config.savedConfigRevision, 'r2');
assert.equal(config.savedConfigText, 'draft');
assert.match(configElements['config-state'].textContent, /重启失败/);
config.dnsPolicyDirty = true;
let prevented = false;
beforeUnload({preventDefault() { prevented = true; }});
assert.equal(prevented, true, 'unloading the page protects unsaved DNS edits');
console.log('PASS: config read errors, no-op save, stale revisions, apply failure and DNS draft protection');
