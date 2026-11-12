import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const page = fs.readFileSync(new URL('../../merlin/webui/sockrocket.asp', import.meta.url), 'utf8');
const source = page.slice(page.indexOf('var srUpdatePollTimer'), page.indexOf('/* ══ Logs'));
function poll(response, disconnected = false) {
  const element = {textContent: ''};
  const timers = [];
  const context = vm.createContext({
    srElement: () => element,
    srApi: (_action, _data, ok, error) => disconnected ? error() : ok(response),
    setTimeout: (callback, delay) => { timers.push({callback, delay}); return timers.length; },
    clearTimeout: () => {}, refreshStatus: () => {},
  });
  new vm.Script(source + '\nsrPollUpdate();').runInContext(context);
  return {text: element.textContent, timers};
}
assert.match(poll({active:false,status:'success: plugin updated'}).text, /更新成功/);
assert.match(poll({active:false,status:'rolled-back: health-check'}).text, /已恢复旧版/);
assert.match(poll({active:false,status:'failed: 已恢复旧版，可以修改设置。网络检查失败，请检查 DNS 和代理节点。'}).text, /已恢复旧版.*网络检查失败/);
assert.match(poll({active:false,status:'rollback-failed: retained'}).text, /备份已保留/);
assert.match(poll({active:false,status:'failed: preparation'}).text, /更新失败/);
const pending = poll({active:true,status:'queued',stage:'replacing'});
assert.match(pending.text, /安装文件/);
assert.equal(pending.timers.length, 1);
const reconnect = poll({}, true);
assert.match(reconnect.text, /等待管理接口恢复/);
assert.equal(reconnect.timers.length, 1);
console.log('PASS: terminal update states, active polling and API reconnect');

for (const [status, label] of [['checking','核对版本'],['downloading','下载软件包'],['verifying','校验软件包']]) {
  const result = poll({active:true,status,job_id:'current'});
  assert.ok(result.text.includes(label));
  assert.equal(result.timers.length, 1);
}
const elements = new Map();
const checkContext = vm.createContext({
  srElement: name => {
    if (!elements.has(name)) elements.set(name, {style:{}});
    return elements.get(name);
  },
  srApi: (_action,_data,ok) => ok({available:true,latest:'1.2.3',asset_url:null,html_url:'release-page'}),
  srShowMsg: () => {},
});
new vm.Script(page.slice(page.indexOf('var pendingUpdateAsset'), page.indexOf('var srUpdatePollTimer')) + '\nsrCheckUpdate();').runInContext(checkContext);
assert.match(elements.get('update-status').textContent, /1\.2\.3.*未找到适配当前机型的软件包/);
assert.equal(elements.get('btn-apply-update').style.display, 'none');
assert.equal(elements.get('btn-update-link').href, 'release-page');
console.log('PASS: preinstall progress and missing platform package');
{
  const element = {textContent: ''};
  const timers = [];
  const ctx = vm.createContext({
    srElement: () => element, clearTimeout: () => {}, setTimeout: cb => timers.push(cb),
    srApi: (_action, _data, ok) => ok({active:false,status:'success: old update',job_id:'old'}),
  });
  new vm.Script(source + "\nsrUpdateJobId = 'current'; srUpdateLaunchDeadline = Date.now() + 30000; srPollUpdate();").runInContext(ctx);
  assert.match(element.textContent, /确认本次更新任务/);
  assert.equal(timers.length, 1);
}
console.log('PASS: stale success cannot complete a newly submitted update');

const initHost = page.slice(page.indexOf('function sockrocketInitHost'), page.indexOf('var TRANSPORT'));
for (const pathname of ['/Module_sockrocket.asp', '/ext/sockrocket/sockrocket.asp']) {
  const notice = {hidden:true}, exit = {href:'/Module_Softcenter.asp#/'}, crumb = {firstChild:{}};
  const document = {form:{current_page:{}, next_page:{}}, getElementById:() => notice, querySelector:() => crumb};
  const context = vm.createContext({document, location:{pathname}, srElement:() => exit});
  new vm.Script(initHost + '\nsockrocketInitHost();').runInContext(context);
  assert.equal(document.form.current_page.value, pathname.split('/').pop());
  assert.equal(notice.hidden, false);
  assert.equal(exit.href, pathname.startsWith('/ext/') ? '/' : '/Module_Softcenter.asp#/');
}
console.log('PASS: Software Center and standalone Merlin navigation without firmware menu scripts');
