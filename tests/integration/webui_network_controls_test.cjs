// Exercise the shipped controls using synthetic state; never touch host networking.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
const elements={},requests=[],messages=[];
function element(id){return elements[id]||=( {textContent:'',disabled:false,className:'',classList:{toggle(){}},setAttribute(){}} );}
const c=vm.createContext({srElement:element,srApi:(...args)=>requests.push(args),srShowMsg:m=>messages.push(m),toastResp:d=>messages.push(d.msg),renderNodes(){},srLoadRules(){},rulePending:false,RULE_READ_PENDING:false,RULE_FLOW_ERROR:false,RULE_RULE_REVISION:null,SR_STATE:{page:'home',statusKnown:true,status:{running:false,transparent_proxy:true,dns_hijack:true}}});
vm.runInContext(source.slice(source.indexOf('var refreshPending'),source.indexOf('function renderChrome()')),c);
vm.runInContext(source.slice(source.indexOf('function setChip('),source.indexOf('/* ══ Service control')),c);
vm.runInContext(source.slice(source.indexOf('var servicePending'),source.indexOf('function toggleService()')),c);
c.renderChrome=()=>c.renderNetworkControls();
const request=action=>{let i=requests.findIndex(r=>r[0]===action);assert.ok(i>=0,action);return requests.splice(i,1)[0];};
const state=(overrides={})=>({running:true,transparent_proxy:true,dns_hijack:true,...overrides});
c.renderNetworkControls();
assert.equal(element('dns-state').textContent,'已关闭');assert.equal(element('fw-state').textContent,'已关闭');
assert.equal(element('chip-dns').disabled,true);assert.equal(element('chip-fw').disabled,true);
assert.equal(c.SR_STATE.status.dns_hijack,true,'displaying Stop must not overwrite saved preferences');
c.toggleDns();c.toggleProxy();assert.equal(requests.length,0,'stopped controls cannot implicitly start the service');
c.SR_STATE.status=state();c.renderNetworkControls();assert.equal(element('chip-dns').disabled,false);
c.refreshStatus();const stale=request('status');request('stats')[2]({});
c.toggleDns();let mutation=request('set_toggles');assert.deepEqual(JSON.parse(JSON.stringify(mutation[1])),{dns_hijack:false});
assert.equal(element('chip-fw').disabled,true);c.toggleProxy();assert.equal(requests.length,0,'independent toggles cannot race each other');
mutation[2]({ok:true,msg:'saved'});assert.equal(element('chip-dns').disabled,true,'keep locked until authoritative status returns');
request('status')[2](state({dns_hijack:false}));
assert.equal(element('dns-state').textContent,'已关闭');assert.equal(element('fw-state').textContent,'已开启');assert.equal(element('chip-fw').disabled,false);
stale[2](state());assert.equal(c.SR_STATE.status.dns_hijack,false,'stale polling must not undo a completed toggle');
request('status')[2](state({dns_hijack:false}));request('stats')[2]({});
c.toggleProxy();mutation=request('set_toggles');assert.deepEqual(JSON.parse(JSON.stringify(mutation[1])),{transparent_proxy:false});
mutation[2]({ok:true});request('status')[2](state({transparent_proxy:false,dns_hijack:false}));
assert.equal(element('dns-state').textContent,'已关闭');assert.equal(element('fw-state').textContent,'已关闭');
c.toggleProxy();mutation=request('set_toggles');assert.deepEqual(JSON.parse(JSON.stringify(mutation[1])),{transparent_proxy:true,dns_hijack:true});
mutation[3]();request('status')[2](state({transparent_proxy:false,dns_hijack:false}));assert.equal(c.networkPending,false);
c.toggleDns();request('set_toggles')[2]({ok:true});request('status')[3]();
assert.equal(c.SR_STATE.statusKnown,false);assert.equal(element('dns-state').textContent,'待确认');assert.equal(element('chip-dns').disabled,true);
c.SR_STATE.statusKnown=true;c.SR_STATE.status=state();c.serviceAction('stop');
assert.equal(element('chip-fw').disabled,true,'service lifecycle locks independent controls');
request('stop')[2]({ok:true,msg:'stopped'});request('status')[2](state({running:false}));request('stats')[2]({});request('sysinfo')[2]({});
assert.equal(element('dns-state').textContent,'已关闭');assert.equal(element('fw-state').textContent,'已关闭');
c.toggleCnDirect();
mutation=request('set_toggles');
assert.deepEqual(JSON.parse(JSON.stringify(mutation[1])),{cn_ipset_direct:true});
mutation[2]({ok:true});request('status')[2](state({running:false,cn_ipset_direct:true}));
assert.equal(element('cn-state').textContent,'已保存，待启用 TUN');
c.SR_STATE.status=state({cn_ipset_direct:true});c.renderNetworkControls();
assert.equal(element('cn-state').textContent,'已开启');
c.toggleCnDirect();mutation=request('set_toggles');
assert.equal(mutation[1].cn_ipset_direct,false);
mutation[2]({ok:true});request('status')[2](state({proxy_mode:'global',cn_ipset_direct:false}));
assert.equal(element('chip-cn').disabled,true);
assert.equal(element('cn-state').textContent,'已关闭');
c.toggleCnDirect();assert.equal(requests.length,0);
// A response from the previous tab cannot invalidate the newly opened editor.
let rulesReloads = 0, networkReloads = 0;
Object.assign(c, {srLoadRules: () => rulesReloads++, loadNetworkPolicy: () => networkReloads++,
  RULE_RULE_REVISION: 'old-rules', dnsPolicyRevision: 'old-network',
  dnsPolicyPending: false, dnsPolicyDirty: false, networkStateText: () => ''});
function poll(page, scope) {
  c.SR_STATE.page = page;
  c.refreshStatus();
  const status = request('status');
  assert.equal(status[1].revision_scope, scope);
  const stats = request('stats');
  assert.equal(stats[1].traffic_scope, page === 'home' ? 'session' : page === 'subs' ? 'monthly' : 'none');
  stats[2]({});
  return status[2];
}
const home = poll('home', 'none');
c.SR_STATE.page = 'dns';
home(state({rules_revision: null, network_revision: null}));
assert.equal(rulesReloads, 0);
poll('dns', 'rules')(state({rules_revision: 'new-rules'}));
assert.equal(rulesReloads, 1);
const fromRules = poll('dns', 'rules');
c.SR_STATE.page = 'network';
fromRules(state({rules_revision: 'new-rules', network_revision: null}));
assert.equal(networkReloads, 0);
poll('network', 'network')(state({network_revision: 'new-network'}));
assert.equal(networkReloads, 1);

// Keep the last table visible while requesting fresh data for the selected tab.
const panels = [];
c.renderSessionTraffic = t => panels.push(['session', t.nodes[0]]);
c.renderMonthTable = (el,keys,rows) => panels.push([el === element('sub-traffic-months') ? 'nodes' : 'subs', rows[0]]);
Object.assign(c, {SR_PAGES:['home','subs','nodes'],SR_PAGE_LOADED:{home:true,subs:true,nodes:true}});
vm.runInContext(source.slice(source.indexOf('function srShowPage('), source.indexOf('/* Structured DNS policy')), c);
function stats(scope) {
  const req = request('stats');
  assert.equal(req[1].traffic_scope, scope);
  return req;
}
c.srShowPage('home');
const staleHome = stats('session');
c.srShowPage('subs');
const oldMonthly = stats('monthly');
c.srShowPage('home');
assert.equal(requests.length, 0, 'avoid a second pending request for the same scope');
staleHome[2]({uptime:'old',traffic:{nodes:['old']}});
assert.deepEqual(panels, []);
stats('session')[2]({uptime:'home',traffic:{nodes:['new']}});
oldMonthly[2]({uptime:'stale',traffic:{recent_month_keys:[],recent_months:['old'],recent_subscriptions:['old']}});
assert.equal(c.SR_STATE.uptime, 'home', 'out-of-order reply must not overwrite newer fields');
assert.deepEqual(panels, [['session','new']]);
c.srShowPage('subs');
// A previous-generation router daemon may still return the complete payload.
stats('monthly')[2]({uptime:'subs',traffic:{nodes:['unused'],recent_month_keys:['2026-11'],recent_months:['monthly node'],recent_subscriptions:['monthly sub']}});
assert.deepEqual(panels.slice(1), [['nodes','monthly node'],['subs','monthly sub']]);
let nodeRenders = 0;
c.renderNodes = () => nodeRenders++;
c.srShowPage('nodes');
stats('none')[2]({uptime:'nodes'});
assert.equal(panels.length, 3, 'uptime-only stats cannot clear traffic tables');
c.acceptStatus(state({active_node:1}));
c.acceptStatus(state({active_node:1}));
assert.equal(nodeRenders, 2, 'entering nodes and changing active node refresh the list, identical polls preserve it');
c.srShowPage('home');
const failedHome = stats('session');
c.srShowPage('nodes');
stats('none')[2]({uptime:'back'});
c.srShowPage('home');
failedHome[3]();
stats('session')[2]({uptime:'fresh',traffic:{nodes:['next']}});
assert.deepEqual(panels.at(-1), ['session','next'], 'returning to a tab retries outdated failed requests');
console.log('PASS: stopped cleanup display, independent toggles, no implicit start, mutation locking, stale polling, error recovery and Stop');
