import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const page = fs.readFileSync(new URL('../../merlin/webui/sockrocket.asp', import.meta.url), 'utf8');
const save = page.slice(page.indexOf('function srSaveConfig()'), page.indexOf("window.addEventListener('beforeunload'"));
for (const error_code of ['timeout', 'network_error']) {
  const elements = new Map();
  const messages = [];
  const ctx = vm.createContext({
    configSaving:false, savedConfigText:'old', savedConfigRevision:'revision', configReadGeneration:0,
    SR_STATE:{statusKnown:true}, utf8b64:s=>s, configEdited:()=>{}, renderChrome:()=>{},
    srElement:name=>{if (!elements.has(name)) elements.set(name,{value:'new'}); return elements.get(name);},
    srApi:(_action,_body,_ok,fail)=>fail({error_code}), srShowMsg:message=>messages.push(message),
  });
  vm.runInContext(save+'\nsrSaveConfig();',ctx);
  assert.equal(ctx.savedConfigText,'old');
  assert.equal(ctx.configSaving,false);
  assert.equal(ctx.SR_STATE.statusKnown,false);
  assert.match(elements.get('config-state').textContent,/保存结果未确认.*局域网 IP.*勿重复保存/);
  assert.equal(messages.length,1);
}
const health = page.slice(page.indexOf('  var watchdog = st.watchdog'), page.indexOf('  var applyLabels'));
for (const [watchdog, failures, expected] of [
  [{outbound_ok:false},0,/连接失败.*不会因此自动转为直连/],
  [{outbound_ok:true,age_secs:3},2,/最近检查通过/],
  [{},2,/连接失败/],
]) {
  const element = {};
  vm.runInNewContext(health,{st:{watchdog},probe:{consecutive_failures:failures},age:5,running:true,SR_STATE:{statusKnown:true},srElement:()=>element});
  assert.match(element.textContent,expected);
}
console.log('PASS: save timeout/disconnect retains draft and warns; failed outbound never claims healthy');
