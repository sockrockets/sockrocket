// Dependency-free smoke checks of prototype rendering and state transitions.
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const html = fs.readFileSync(`${__dirname}/index.html`, 'utf8');
const source = html.split('<script>')[1].split('</script>')[0];
const elements = new Map();
function element(id) {
  if (!elements.has(id)) elements.set(id, {
    innerHTML: '', textContent: '', value: '', open: false, disabled: false,
    classList: { toggle() {}, add() {}, remove() {} },
    checked: true, files: [], addEventListener() {},
    setAttribute() {}, focus() {}, setSelectionRange() {},
    showModal() { this.open = true; }, close() { this.open = false; },
  });
  return elements.get(id);
}
const timers = [];
let checkedMembers = [];
const context = vm.createContext({
  document: {
    querySelector: element, querySelectorAll: () => checkedMembers.map(value => ({value})),
    addEventListener() {}, body: element('body'),
  },
  setTimeout(fn) { timers.push(fn); return timers.length; }, clearTimeout() {},
  matchMedia: () => ({ matches: false }), navigator: {}, console, URL,
});
vm.runInContext(source, context);
const evalCode = s => vm.runInContext(s, context);
async function action(name, value = '') {
  await evalCode(`run(${JSON.stringify(name)},${JSON.stringify(value)},{})`);
}
(async () => {
  for (const page of ['home','nodes','subscriptions','groups','rules','connections','logs','network','general']) {
    await action('nav', page);
    assert(element('#main').innerHTML.includes('<h1>'), page);
    assert(!element('#main').innerHTML.includes('undefined'), page);
  }
  for (const dialog of ['mode','import','group','rule','rule-import','presets','rule-test','tun','dns','outbound','share','share-server','appearance','language','shortcuts','config','clear-logs']) {
    await action(dialog);
    assert(element('#dialog').open, dialog);
    assert(element('#dialog').innerHTML.includes('dialog-title'), dialog);
    await action('close');
  }
  evalCode('bulkDraft=null');
  const parsed = evalCode("parseRuleText('# comment\\nDOMAIN-SUFFIX,example.net,PROXY\\nGEOIP,cn,DIRECT\\nDST-PORT,8000-9000,自动选择\\nMATCH,REJECT')");
  assert.equal(parsed.entries.length, 3);
  assert.equal(parsed.entries[1].value, 'CN');
  assert.equal(parsed.final.action, '拒绝');
  assert.equal(parsed.errors.length, 0);
  for (const [line, expected] of [
    ['DST-PORT,9000-8000,DIRECT', '端口'],
    ['DST-PORT,65536,DIRECT', '端口'],
    ['IP-CIDR,999.1.1.1/24,DIRECT', '网段'],
    ['IP-CIDR,2001:db8::/129,DIRECT', '网段'],
    ['DOMAIN,https://example.com,PROXY', '域名'],
    ['GEOIP,CHINA,DIRECT', '两位'],
    ['DOMAIN,example.net,unknown-group', '出口不存在'],
    ['MATCH,PROXY\nDOMAIN,example.net,PROXY', '必须放在'],
    ['MATCH,PROXY\nMATCH,DIRECT', '只能有一条'],
  ]) {
    const result = evalCode(`parseRuleText(${JSON.stringify(line)})`);
    assert(result.errors.some(e => e.message.includes(expected)), line);
  }
  assert.equal(evalCode("parseRuleText('IP-CIDR,2001:db8::/32,DIRECT').errors.length"), 0);
  // A preview never mutates saved rules, and skip/overwrite differ in outcome.
  evalCode("startBulk('append');bulkDraft.text='DOMAIN-SUFFIX,example.com,DIRECT\\nDOMAIN,new.example.net,REJECT\\nDOMAIN,new.example.net,PROXY'");
  const before = evalCode('JSON.stringify(state.rules)');
  let plan = evalCode('planBulk()');
  assert.equal(plan.added, 1);
  assert.equal(plan.skipped, 2);
  assert.equal(plan.rows.find(r => r[2] === 'example.com')[3], '代理');
  assert.equal(evalCode('JSON.stringify(state.rules)'), before);
  evalCode("bulkDraft.policy='覆盖已有'");
  plan = evalCode('planBulk()');
  assert.equal(plan.rows.find(r => r[2] === 'example.com')[3], '直连');
  assert.equal(plan.rows.filter(r => r[2] === 'new.example.net').length, 1);
  assert.equal(plan.rows.find(r => r[2] === 'new.example.net')[3], '代理');
  // Group editing replaces only its scope and keeps its insertion position.
  evalCode("startBulk('group','跨境访问');bulkDraft.text='DOMAIN,edited.example.net,REJECT'");
  plan = evalCode('planBulk()');
  assert.equal(plan.removed, 2);
  assert.equal(plan.rows.filter(r => r[0] === '跨境访问').length, 1);
  assert.equal(plan.rows.filter(r => r[0] === '国内直连').length, 3);
  assert.equal(plan.rows[3][2], 'edited.example.net');
  // Commit uses the actual pasted content rather than a canned example.
  element('#rule-bulk').value='DOMAIN,edited.example.net,REJECT\nMATCH,DIRECT';
  element('#bulk-group').value='跨境访问';
  element('#rule-dup').value='跳过重复';
  await action('bulk-apply');
  assert.equal(evalCode("state.rules[3][2]"), 'edited.example.net');
  assert.equal(evalCode('state.finalAction'), '直连');
  assert.equal(evalCode('bulkDraft'), null);
  const savedCount = evalCode('state.rules.length');
  await action('rule', 'final');
  element('#rule-action').value='拒绝';
  await action('save-rule', 'final');
  assert.equal(evalCode('state.finalAction'), '拒绝');
  assert.equal(evalCode('state.rules.length'), savedCount);
  await action('rule-view', '0');
  assert(element('#dialog').innerHTML.includes('GEOIP'));
  await action('rule-move', '0|1');
  assert.equal(evalCode('state.rules[1][1]'), 'GEOIP');
  await action('rule-toggle', '1');
  assert.equal(evalCode('state.rules[1][4]'), false);
  await action('confirm-delete-rule', '1');
  assert.equal(evalCode('state.rules.length'), savedCount-1);
  await action('select-node', '日本 02');
  assert.equal(evalCode('state.node'), '日本 02');
  await action('scenario', 'first');
  assert(element('#main').innerHTML.includes('准备好，连接更简单'));
  await action('scenario', 'error');
  assert(element('#main').innerHTML.includes('连接超时'));
  await action('scenario', 'ready');
  await action('connect');
  assert.equal(evalCode('state.connected'), false);
  assert.equal(evalCode('state.system'), false);
  await action('connect');
  assert.equal(evalCode('state.connecting'), true);
  timers.splice(0).forEach(fn => fn());
  assert.equal(evalCode('state.connected'), true);
  await action('nav', 'network');
  element('#listen').value='127.0.0.1';
  element('#socks').value='70000'; element('#http').value='8080';
  await action('apply-network');
  assert.match(element('#network-error').textContent, /1–65535/);
  element('#socks').value='1080'; element('#http').value='1080';
  await action('apply-network');
  assert.match(element('#network-error').textContent, /不能相同/);
  element('#http').value='8080';
  await action('apply-network');
  assert.equal(evalCode('state.dirty'), false);
  evalCode('state.dirty=true');
  await action('nav', 'home');
  assert.equal(evalCode('state.page'), 'network');
  assert(element('#dialog').innerHTML.includes('尚未应用更改'));
  // Product semantics: running a listener does not imply traffic capture.
  evalCode("state.connected=true;state.system=false;state.tun=false;state.error=false");
  assert(evalCode('homeContent()').includes('尚未接管系统流量'));
  assert(evalCode('homeContent()').includes('尚未验证'));
  await action('enable-system');
  assert.equal(evalCode('state.system'), true);
  // Filtering, favorites, and sorting compose rather than changing selection.
  evalCode("state.nodeSourceFilter='日常线路';state.nodeQuery='';state.favoritesOnly=false");
  assert.equal(evalCode('visibleNodes().length'), 3);
  evalCode('state.favoritesOnly=true');
  assert.equal(evalCode('visibleNodes().length'), 1);
  await action('favorite-node','日本 02');
  assert.equal(evalCode('visibleNodes().length'), 2);
  evalCode("state.nodeQuery='日本'");
  assert.equal(evalCode('visibleNodes()[0][0]'), '日本 02');
  await action('reset-node-filters');
  assert.equal(evalCode('visibleNodes().length'), 4);
  assert.equal(evalCode("groupPick(state.groups.find(g=>g.id==='fallback'))"), '日本 02');
  assert.equal(evalCode("groupPick(state.groups.find(g=>g.id==='current'))"), '香港 01');
  // Undo restores the previous real rule snapshot exactly once.
  const beforeUndo=evalCode('JSON.stringify(state.rules)');
  await action('rule-toggle','0');
  assert.notEqual(evalCode('JSON.stringify(state.rules)'), beforeUndo);
  await action('rules-undo');
  assert.equal(evalCode('JSON.stringify(state.rules)'), beforeUndo);
  assert.equal(evalCode('state.ruleUndo'), null);
  // Discard returns to the last successfully applied values, not hard-coded defaults.
  evalCode("state.lastApplied={listen:'127.0.0.2',socks:'1180',http:'8180',lan:false};state.listen='0.0.0.0';state.socks='9999';state.dirty=true");
  await action('discard-nav','home');
  assert.equal(evalCode('state.socks'),'1180');
  assert.equal(evalCode('state.listen'),'127.0.0.2');
  assert.equal(evalCode('state.dirty'),false);
  await action('diagnostics');
  assert(element('#dialog').innerHTML.includes('目标连通性与 DNS'));
  assert(element('#dialog').innerHTML.includes('不发送真实探测'));
  await action('close');
  // Renaming a group updates references instead of leaving invalid rule exits.
  evalCode("state.rules[0][3]='自动选择';state.finalAction='自动选择'");
  element('#group-name').value='自动线路';element('#group-policy').value='URL 测速';
  element('#group-picked').value='香港 01';checkedMembers=['香港 01','日本 02'];
  await action('save-group','current');
  assert.equal(evalCode('state.rules[0][3]'),'自动线路');
  assert.equal(evalCode('state.finalAction'),'自动线路');
  assert.equal(evalCode("ruleActions().includes('自动选择')"),false);
  assert.equal(evalCode("ruleActions().includes('自动线路')"),true);
  evalCode("state.rules.push(['停用组','DOMAIN','paused.example.net','代理',false]);startBulk('group','停用组')");
  assert.equal(evalCode("planBulk().rows.find(r=>r[2]==='paused.example.net')[4]"),false);
  evalCode('bulkDraft=null');await action('close');
  const firstUpdate=evalCode('state.subs[0][2]');
  await action('retry-sub','1');
  assert.equal(evalCode('state.subUpdates.has(0)'),false);
  assert.equal(evalCode('state.subUpdates.has(1)'),true);
  timers.splice(0).forEach(fn=>fn());
  assert.equal(evalCode('state.subs[0][2]'),firstUpdate);
  assert.equal(evalCode('state.subs[1][2]'),'刚刚');
  console.log('PASS: page/dialog rendering, rule workflows and validation, service/capture semantics, source filtering, favorites, strategy selection, undo, saved-config recovery and diagnostics');
})().catch(error => { console.error(error); process.exitCode = 1; });
