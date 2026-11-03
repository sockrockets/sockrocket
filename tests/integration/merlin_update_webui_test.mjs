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
assert.match(poll({active:false,status:'rollback-failed: retained'}).text, /备份已保留/);
assert.match(poll({active:false,status:'failed: preparation'}).text, /更新失败/);
const pending = poll({active:true,status:'queued',stage:'replacing'});
assert.match(pending.text, /安装文件/);
assert.equal(pending.timers.length, 1);
const reconnect = poll({}, true);
assert.match(reconnect.text, /等待管理接口恢复/);
assert.equal(reconnect.timers.length, 1);
console.log('PASS: terminal update states, active polling and API reconnect');
