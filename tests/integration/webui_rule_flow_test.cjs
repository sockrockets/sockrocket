// Exercise the shipped UI functions with synthetic configuration only.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
function fixture() {
  const elements={},requests=[],inspected=[],edited=[];
  const element=id=>elements[id]||=( {value:'',checked:true,hidden:false,style:{},parentElement:{style:{}},innerHTML:'',textContent:'',setAttribute(){},focus(){},scrollIntoView(){}} );
  const context=vm.createContext({confirm:()=>true,toastResp(){},SR_PROXY_GROUPS:[],setProxyGroupTargets(){},renderDevicePolicies(){},srElement:element,srApi:(...args)=>requests.push(args),srEsc:v=>String(v??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),srShowMsg(){},inspectRule:i=>inspected.push(i),editRule:i=>edited.push(i),SR_PAGE_LOADED:{},SR_STATE:{statusKnown:true,status:{running:true,proxy_mode:'rule',active_node:0},nodes:[{name:'Example node'}],ruleRows:[]}});
  vm.runInContext(source.slice(source.indexOf('var RULE_TYPE_LABEL'),source.indexOf('var ruleExportPending')), context);
  context.inspectRule=i=>inspected.push(i);context.editRule=i=>edited.push(i);
  context.RULE_RULE_REVISION='revision';element('rule-flow-device').value='global';
  const request=action=>{const at=requests.findIndex(r=>r[0]===action);assert.ok(at>=0,action);return requests.splice(at,1)[0];};
  return {context,element,request,requests,inspected,edited};
}
const rule=(index,overrides={})=>({index,source:'',name:'',group:'',rule_type:'domain',pattern:`site${index}.invalid`,target:'proxy',priority:0,enabled:true,...overrides});
const result=(row,overrides={})=>({ok:true,revision:'revision',mode:'rule',destination:'example.com',port:443,source:null,active_node:0,node_name:'Example node',running:true,transparent_proxy:true,cn_ipset_direct:false,trace:{action:row.target,matched_rule:row.index,matched_builtin:false,matched_snapshot:Object.fromEntries(Object.entries(row).filter(([k])=>k!=='index')),definitive:true,dns:'not_needed',steps:[{config_index:row.index,rule_type:row.rule_type,outcome:'matched',checked:1}],checked_rules:1,omitted_steps:0},...overrides});
const f=fixture(),c=f.context;
c.SR_STATE.ruleRows=Array.from({length:12},(_,i)=>rule(i));
assert.equal(c.ruleFlowModel(c.SR_STATE.ruleRows,'rule','')[0].count,12,'all rules are represented');
c.SR_STATE.ruleRows.push(rule(12,{rule_type:'final',source:'192.0.2.8',target:'direct'}));
let stages=c.ruleFlowModel(c.SR_STATE.ruleRows,'rule','192.0.2.8');
assert.equal(stages.at(-1).kind,'device_final');
assert.ok(!stages.some(stage=>stage.kind==='global_rule'));
assert.ok(stages.some(stage=>stage.kind==='builtin'));
assert.ok(!stages.some(stage=>stage.kind==='global_final'));
assert.ok(!c.ruleFlowModel(c.SR_STATE.ruleRows,'rule','').some(r=>r.kind==='device_final'),'global simulation skips device stages');
f.element('rule-flow-destination').value='example.com';c.traceRuleFlow();
let req=f.request('trace_route');assert.equal(req[1].expected_revision,'revision');assert.equal(req[1].port,undefined);
req[2](result(c.SR_STATE.ruleRows[9]));c.flowMatchedRule(true);
assert.deepEqual(f.edited,[9],'exact winning index after the old four-rule cutoff');
assert.match(f.element('rule-flow').innerHTML,/配置 #10/);
c.renderRuleFlow();assert.equal(f.requests.length,0,'status rendering must not trigger DNS queries');
c.SR_STATE.ruleRows[9].pattern='changed.invalid';c.flowMatchedRule(true);
assert.deepEqual(f.edited,[9],'stale snapshot must not edit another rule');
f.request('get_rules')[2]({ok:true,rules:c.SR_STATE.ruleRows,revision:'next'});
assert.equal(c.RULE_RULE_REVISION,'next');
c.traceRuleFlow();req=f.request('trace_route');
f.element('rule-flow-destination').value='other.invalid';c.invalidateRuleTrace();
req[2](result(c.SR_STATE.ruleRows[1]));assert.equal(c.RULE_TRACE,null,'changed inputs discard old response');
assert.equal(f.element('rule-flow-check').disabled,false);
c.traceRuleFlow();req=f.request('trace_route');c.srLoadRules();
f.request('get_rules')[2]({ok:true,rules:c.SR_STATE.ruleRows,revision:'newest'});
req[2](result(c.SR_STATE.ruleRows[1]));assert.equal(c.RULE_TRACE,null,'refresh while pending invalidates response');
c.showFlowRuleList('device_final');assert.match(f.element('rule-count').textContent,/当前显示 13 条/,'same unnamed group spans all phases');
c.showFlowRuleList();assert.match(f.element('rule-count').textContent,/当前显示 13 条/);
const unsafe=rule(20,{name:'<img src=x onerror=alert(1)>'});c.SR_STATE.ruleRows=[unsafe];
const provisional=result(unsafe);provisional.trace.definitive=false;provisional.trace.dns='failed';
const html=c.flowTraceHtml(provisional);
assert.doesNotMatch(html,/<img/);assert.match(html,/&lt;img/);assert.match(html,/待验证/);assert.match(html,/前序 IP 条件待验证/);
c.srLoadRules();f.request('get_rules')[3]();assert.equal(c.RULE_TRACE,null);assert.equal(c.RULE_FLOW_ERROR,true);
const before=f.requests.length;c.traceRuleFlow();assert.equal(f.requests.length,before,'failed rule load cannot produce apparently current results');
const switched=fixture();switched.element('rule-flow-destination').value='example.com';switched.context.traceRuleFlow();
const pending=switched.request('trace_route');switched.context.SR_STATE.status.proxy_mode='direct';switched.context.renderRuleFlow();
pending[2](result(rule(0)));assert.equal(switched.context.RULE_TRACE,null,'mode change during DNS lookup invalidates pending result');
assert.equal(switched.element('rule-flow-check').disabled,false);
console.log('PASS: complete phases, device final, exact winner editing, stale inputs/config, list filters, provisional DNS and HTML escaping');

const grouped=fixture(),g=grouped.context;
g.SR_STATE.ruleRows=[rule(0,{group:'Local',target:'direct',priority:50}),rule(1,{group:'Region',rule_type:'geoip',pattern:'CN',enabled:false,priority:40}),rule(2,{group:'AI',priority:10}),rule(3,{group:'Local',target:'reject',priority:0}),rule(4,{group:'Fallback',rule_type:'final',priority:1000})];
const model=g.ruleFlowModel(g.SR_STATE.ruleRows,'rule','');
assert.deepEqual(Array.from(model.filter(r=>r.indices).flatMap(r=>Array.from(r.indices))),[0,1,2,3,4],'every configured rule appears in effective order, including disabled rules and interleaved groups');
assert.equal(model[1].disabled,true);assert.equal(model[1].title,'Region');
g.renderRuleFlow();assert.match(grouped.element('rule-flow-summary').textContent,/共 5 条 · 启用 4 条 · 停用 1 条/);
g.showFlowRuleList('global_rule',1);assert.match(grouped.element('rule-count').textContent,/当前显示 1 条/);assert.equal(g.RULE_SELECTED_GROUP,'Region');
g.SR_RULE_NODES=[{id:'ab'.repeat(32),name:'Example B'}];g.renderRules(false);
g.SR_STATE.status.transparent_proxy=true;g.SR_STATE.status.cn_ipset_direct=false;
g.RULE_TRACE=result(g.SR_STATE.ruleRows[3]);g.RULE_FLOW_SELECTED=-1;g.renderRuleFlow();assert.equal(g.RULE_FLOW_SELECTED,3,'highlight the exact matching group segment, not the first global group');
let graph=grouped.element('rule-flow').innerHTML;
let highlighted=Array.from(graph.matchAll(/<path[^>]* class="selected[^"]*" d="([^"]+)"/g),m=>m[1]);
assert.equal(highlighted.length,2,'only source-to-winner and winner-to-outlet form the highlighted route');
assert.match(graph,/<path data-from="s0" data-to="g0" class="selected"/,'the matched Local rule highlights the shared Local node');
assert.doesNotMatch(graph,/<path data-from="g1"/,'disabled group has no outlet');
g.selectRuleFlow(0);
assert.deepEqual(Array.from(grouped.element('rule-flow').innerHTML.matchAll(/<path[^>]* class="selected[^"]*" d="([^"]+)"/g),m=>m[1]),highlighted,'inspecting another group cannot change the actual highlighted route');
g.SR_STATE.status={...g.SR_STATE.status,active_node:0,node_count:1};g.renderRuleFlow();
assert.notEqual(g.RULE_TRACE,null,'normal status polling must preserve a valid trace on the default node');
g.srLoadRules();const oldRead=grouped.request('get_rules');g.srLoadRules();
grouped.request('get_rules')[2]({ok:true,rules:[rule(0,{group:'Fresh'})],revision:'fresh',proxy_mode:'direct',active_node_name:'Snapshot B'});
oldRead[2]({ok:true,rules:[rule(0,{group:'Stale'})],revision:'stale',proxy_mode:'rule'});
assert.equal(g.SR_STATE.ruleRows[0].group,'Fresh');assert.equal(g.RULE_SNAPSHOT_MODE,'direct');
assert.equal(g.ruleActiveNodeName(),'Snapshot B','rule snapshot wins over an older status response');
g.SR_STATE.status.active_node=1;
assert.equal(g.ruleActiveNodeName(),'Snapshot B','a subsequent status response cannot mix snapshot revisions');
oldRead[3]();assert.equal(g.RULE_FLOW_ERROR,false,'stale failures cannot replace the fresh snapshot');
assert.match(grouped.element('rule-flow').innerHTML,/全部直连/);
console.log('PASS: diagram/list group identity, disabled groups, priority segments, exact winner and out-of-order reads');

const mixed=fixture(),m=mixed.context;
m.SR_STATE.ruleRows=[rule(0,{group:'Mixed',target:'proxy'}),rule(1,{group:'Mixed',target:'direct',enabled:false})];
let mixedGroup=m.ruleFlowModel(m.SR_STATE.ruleRows,'rule','')[0];
assert.deepEqual(Array.from(mixedGroup.targets),['proxy'],'a partially disabled group must expose only enabled outlets');
assert.deepEqual(Array.from(mixedGroup.indices),[0,1],'disabled members remain available for diagram/list correspondence');
m.SR_STATE.ruleRows[0].enabled=false;mixedGroup=m.ruleFlowModel(m.SR_STATE.ruleRows,'rule','')[0];
assert.equal(mixedGroup.disabled,true);m.renderRuleFlow();
assert.doesNotMatch(mixed.element('rule-flow').innerHTML,/<path data-from="g0"/,'fully disabled groups must have no outbound route');
console.log('PASS: partially disabled groups expose only effective outlets; fully disabled groups have no route');

{
 const f=fixture(),c=f.context;
 c.SR_STATE.ruleRows=[rule(0,{group:'Phone',source:'192.0.2.10'}),rule(1,{group:'Laptop',source:'192.0.2.11'}),rule(2,{group:'Shared'}),rule(3,{group:'Fallback',rule_type:'final'})];
 c.renderRuleFlow();
 const data=f.element('rule-flow').innerHTML;
 assert.equal((data.match(/class="map-node source/g)||[]).length,3,'two devices and all devices share one source column');
 assert.equal((data.match(/class="routing-map"/g)||[]).length,1);
 assert.equal((data.match(/<strong>Shared<\/strong>/g)||[]).length,1);

}

{
 const f=fixture(),c=f.context;
 c.SR_PROXY_GROUPS=[{name:'Video',selected_node:'Example B'},{name:'Work',selected_node:'Example C'}];
 c.RULE_TARGET_LABEL['group:Video']='Video';c.RULE_TARGET_LABEL['group:Work']='Work';
 c.SR_STATE.ruleRows=[rule(0,{group:'Video rules',target:'group:Video'}),rule(1,{group:'Work rules',target:'group:Work'})];
 f.element('rule-flow-destination').value='example.com';c.traceRuleFlow();
 f.request('trace_route')[2](result(c.SR_STATE.ruleRows[0],{proxy_group:'Video',node_name:'Example B'}));
 const html=f.element('rule-flow').innerHTML;
 assert.match(html, /出口成员：Example B/);
 assert.match(html, /class="map-node outlet selected"[^>]*><strong>Video<\/strong>/);
 assert.doesNotMatch(html, /<div class="flow-box"><strong>拒绝<\/strong>/,'an unused reject exit is not shown');
 assert.doesNotMatch(html, /\[object Object\]/);
 console.log('PASS: group trace highlights the exact exit and expands the diagram for additional groups');
}

{
 const f=fixture(), c=f.context, el=f.element;
 const tv='192.0.2.8', laptop='192.0.2.9';
 c.SR_STATE.devices=[{source:tv+'/32',alias:'客厅电视'},{source:laptop+'/32',host:'Laptop'}];
 c.SR_STATE.ruleRows=[rule(0,{group:'Emby',target:'direct'})];
 c.SR_STATE.deviceBindings=[{source:tv+'/32',group:'Emby',target:'proxy'},{source:laptop+'/32',group:'Emby',target:'reject'}];
 el('rule-flow-device').value='';c.syncFlowDevicePicker();c.renderRuleFlow();
 const html=el('rule-flow').innerHTML;
 assert.match(html,/客厅电视/);assert.match(html,/Laptop/);
 for(const label of ['Emby','直连','代理','拒绝']){
   assert.equal(html.split('<strong>'+label+'</strong>').length-1,1,label+' is drawn exactly once');
 }
 el('rule-flow-destination').value='video.invalid';c.traceRuleFlow();assert.equal(f.requests.length,0);
 assert.equal((html.match(/class="routing-map"/g)||[]).length,1);
 assert.equal((html.match(/class="map-node source/g)||[]).length,3);
 c.chooseGlobalFlow();c.traceRuleFlow();assert.equal(f.request('trace_route')[1].source,'');
 c.chooseFlowHost(tv);assert.match(el('rule-flow').innerHTML,/设备关联组/);assert.match(el('rule-flow').innerHTML,/全局自定义规则/);
 c.traceRuleFlow();let first=f.request('trace_route');
 assert.equal(first[1].source,tv);assert.match(el('rule-flow').innerHTML,/Laptop/,'selecting the TV keeps the other device visible');
 c.chooseFlowHost(laptop);first[2](result(c.SR_STATE.ruleRows[0],{source:tv}));assert.equal(c.RULE_TRACE,null);
 c.traceRuleFlow();assert.equal(f.request('trace_route')[1].source,laptop);
 el('rule-flow-device').value='global';c.changeFlowDevice();c.traceRuleFlow();assert.equal(f.request('trace_route')[1].source,'');
 el('rule-flow-device').value='manual';c.changeFlowDevice();c.traceRuleFlow();assert.equal(f.requests.length,0);
 el('rule-flow-client').value='2001:db8::7';c.invalidateRuleTrace();c.traceRuleFlow();assert.equal(f.request('trace_route')[1].source,'2001:db8::7');
 c.resetDeviceFlow();assert.match(el('rule-flow').innerHTML,/routing-map/);
 console.log('PASS: parallel device branches, explicit simulation source, per-device exits, stale replies and reset');
}

{
 const f=fixture(), c=f.context, el=f.element, host='192.0.2.8';
 const rows=[rule(0,{group:'Media',target:'DIRECT'}),rule(1,{group:'Unused',target:'reject'}),rule(2,{group:'Default',rule_type:'FINAL',target:'direct'})];
 c.SR_STATE.ruleRows=rows;
 c.SR_STATE.compiledTargets=['direct',null,'direct'];
 c.SR_STATE.status.transparent_proxy=true;
 c.SR_STATE.deviceBindings=[{group:'Media',source:'192.0.2.0/24',target:'reject'},{group:'Media',source:host+'/32',target:'proxy'}];
 el('rule-flow-device').value='';c.renderRuleFlow();
 const overview=el('rule-flow').innerHTML;assert.doesNotMatch(overview,/Unused/);
 assert.match(overview,/全局规则[\s\S]*?Media[\s\S]*?直连/);
 const netRoute=c.ruleFlowModel(rows,'rule','192.0.2.0/24').find(s=>s.kind==='device_link');
 assert.deepEqual([...netRoute.targets],['reject'],'subnet summary does not inherit a narrower host override');
 c.chooseFlowHost(host);
 const route=c.ruleFlowModel(rows,'rule',host);assert.deepEqual([...route.find(s=>s.kind==='device_link').targets],['proxy']);
 assert.ok(!route.some(s=>s.kind==='global_rule'));
 assert.deepEqual([...c.ruleFlowModel(rows,'rule','198.51.100.7').find(s=>s.kind==='global_rule').targets],['direct']);
 const match=source=>({config_index:0,source,rule_type:'domain',outcome:'matched',checked:1});
 const reply=result(rows[0],{source:host});reply.trace.steps=[match(host+'/32')];reply.trace.action='proxy';
 c.RULE_TRACE=reply;c.RULE_FLOW_SELECTED=-1;c.renderRuleFlow();assert.equal(route[c.RULE_FLOW_SELECTED].kind,'device_link');
 console.log('PASS: inherited and global outlets, overlapping sources and backend-selected phase');
}

{
 const f=fixture(), c=f.context, el=f.element, host='192.0.2.8', subnet='192.0.2.0/24';
 const rows=[rule(0,{group:'Media',priority:100,target:'direct'}),rule(1,{group:'Japan',target:'proxy'})];
 const links=[{source:host,group:'Media'},{source:host,group:'Japan',priority:20,target:'reject'},{source:subnet,group:'Japan',target:'direct'}];
 const refresh=(alias)=>{
   c.srLoadRules();f.request('get_rules')[2]({ok:true,revision:'revision',rules:rows,device_rule_groups:links,
     devices:[{source:host+'/32',alias,host:'TV'},{source:subnet,alias:'影音网段'}]});
 };
 refresh('旧名称');c.chooseFlowHost(host);
 refresh('客厅屏幕');
 assert.equal(el('rule-flow-device').value,host);
 assert.match(el('rule-flow').innerHTML,/<strong>客厅屏幕<\/strong>/);
 assert.doesNotMatch(el('rule-flow').innerHTML,/旧名称/);
 assert.match(el('rule-flow').innerHTML,/<strong>影音网段<\/strong>/);
 assert.match(el('rule-flow-device').innerHTML,/客厅屏幕/);
 const stages=c.ruleFlowModel(rows,'rule',host).filter(s=>s.kind==='device_link');
 assert.deepEqual(Array.from(stages,s=>s.title),['Japan','Media']);
 assert.match(el('rule-flow').innerHTML,/<strong>Japan<\/strong><small>关联优先级 20/);
 c.SR_STATE.status.transparent_proxy=true;
 const reply=result(rows[1],{source:host});reply.trace.action='reject';reply.trace.steps[0].source=host;
 c.RULE_TRACE=reply;c.RULE_FLOW_SELECTED=-1;c.renderRuleFlow();
 assert.match(el('rule-flow').innerHTML,/class="map-node outlet selected"[^>]*><strong>拒绝/);
 refresh('');assert.match(el('rule-flow').innerHTML,/<strong>TV<\/strong>/);
 console.log('PASS: device priority, matching exit and refreshed device and subnet names on one graph');
}

// Read the rendered wires and their endpoints independently of the flow model.
function deviceConnections(html, source, hitsOnly=false) {
 const sources=Array.from(html.matchAll(/data-source="([^"]*)" onclick="pickMapSource/g),m=>m[1]);
 const groups=Array.from(html.matchAll(/class="map-node group[^"]*"[^>]*><strong>([^<]*)<\/strong>/g),m=>m[1]);
 const index=sources.indexOf(source);
 assert.ok(index>=0,'source is present on the shared canvas');
 return Array.from(html.matchAll(/<path data-from="s(\d+)" data-to="g(\d+)" class="([^"]*)"/g))
   .filter(m=>Number(m[1])===index && (!hitsOnly||m[3]==='selected'))
   .map(m=>groups[Number(m[2])]).sort();
}

{
 const f=fixture(),c=f.context,el=f.element, tv='192.0.2.8', network='192.0.2.0/24', other='198.51.100.9';
 const rows=[rule(0,{group:'Media'}),rule(1,{group:'Work'}),rule(2,{group:'LAN'}),
   rule(3,{source:tv,group:'Exception'}),rule(4,{source:tv,group:'TV default',rule_type:'final'}),
   rule(5,{group:'Default',rule_type:'final'}),rule(6,{source:tv,group:'Off',enabled:false})];
 c.SR_STATE.ruleRows=rows;
 c.SR_STATE.deviceBindings=[{source:tv+'/32',group:'Media',target:'direct'},{source:network,group:'LAN',target:'reject'}];
 c.SR_STATE.devices=[{source:other,alias:'Guest'}];
 el('rule-flow-device').value='';c.renderRuleFlow();
 const diagram=()=>el('rule-flow').innerHTML;
 assert.deepEqual(deviceConnections(diagram(),tv),['Exception','Media','TV default']);
 assert.deepEqual(deviceConnections(diagram(),network),['LAN','设备默认出口']);
 assert.deepEqual(deviceConnections(diagram(),other),[]);
 assert.deepEqual(deviceConnections(diagram(),''),['Default','LAN','Media','Work','内置国内 / 局域网']);
 c.chooseFlowHost(tv);c.SR_STATE.status.transparent_proxy=true;
 for(const [index,owner,action] of [[1,'','proxy'],[2,network,'reject'],[0,tv,'direct'],[3,tv,'proxy'],[4,tv,'proxy']]){
   const reply=result(rows[index],{source:owner?tv:other});
   c.chooseFlowHost(owner?tv:other);
   reply.trace.steps[0].source=owner===tv?tv+'/32':owner||null;
   reply.trace.action=action;c.RULE_TRACE=reply;c.renderRuleFlow();
   assert.deepEqual(deviceConnections(diagram(),owner,true),[rows[index].group]);
   assert.equal((diagram().match(/<path[^>]*class="selected"/g)||[]).length,2);
   assert.deepEqual(deviceConnections(diagram(),tv),['Exception','Media','TV default']);
 }
 c.RULE_TRACE=null;c.RULE_SNAPSHOT_MODE='direct';c.renderRuleFlow();
 assert.deepEqual(deviceConnections(diagram(),tv),[]);
 assert.deepEqual(deviceConnections(diagram(),''),['全部直连']);
 console.log('PASS: only configured device wires are drawn; host, subnet and global hits use their owning source');
}

{
 const f=fixture(),c=f.context,host='192.0.2.8';
 const rules=[rule(0,{group:'Imported'}),rule(1,{group:'Legacy'}),rule(2,{group:'Unbound'})];
 const bindings=[{source:'',group:'Legacy',target:'direct'}];
 const refresh=()=>{c.srLoadRules();f.request('get_rules')[2]({ok:true,rules,global_rule_groups:bindings,device_rule_groups:[{source:host,group:'Imported',target:'reject'}]});};
 refresh();
 const graph=()=>f.element('rule-flow').innerHTML;
 assert.deepEqual(deviceConnections(graph(),host),['Imported','设备默认出口']);
 assert.deepEqual(deviceConnections(graph(),''),['Legacy','全局设备兜底','内置国内 / 局域网']);
 bindings.push({source:'',group:'Imported',target:'proxy',priority:10});refresh();
 assert.equal(c.ruleFlowModel(rules,'rule',host).some(s=>s.kind==='global_rule'),false);
 const globals=c.ruleFlowModel(rules,'rule','198.51.100.9').filter(s=>s.kind==='global_rule');
 assert.deepEqual(Array.from(globals,s=>[s.title,s.targets[0]]),[['Imported','proxy'],['Legacy','direct']]);
 assert.doesNotMatch(graph(),/<strong>Unbound<\/strong>/);
}

{
 const f=fixture(), c=f.context;
 const rows=[rule(0,{target:'direct'}),rule(1,{rule_type:'final',target:'proxy'})];
 c.SR_STATE.ruleRows=rows;
 c.SR_STATE.status.transparent_proxy=true;
 const graph=()=>f.element('rule-flow').innerHTML;
 c.renderRuleFlow();
 assert.deepEqual(deviceConnections(graph(),''),['全局设备兜底','内置国内 / 局域网','未分组'].sort());
 assert.match(graph(), /data-from="g0" data-to="e0"/);
 assert.doesNotMatch(graph(), /data-from="g0" data-to="e1"/,'fallback proxy is not an outlet of the direct rule');
 assert.match(graph(), /data-from="g2" data-to="e1"/);
 for(const [i,group,outlet] of [[0,'未分组','直连'],[1,'全局设备兜底','代理']]){
   c.RULE_TRACE=result(rows[i]);c.renderRuleFlow();
   assert.deepEqual(deviceConnections(graph(),'',true),[group]);
   assert.match(graph(), new RegExp('class="map-node outlet selected"[^>]*><strong>'+outlet+'</strong>'));
   assert.equal((graph().match(/<path[^>]*class="selected"/g)||[]).length,2);
 }
 for(const finals of [[],[rule(0,{rule_type:'final',group:'设备默认策略',target:'direct'})]]){
   c.SR_STATE.ruleRows=finals;c.RULE_TRACE=null;c.renderRuleFlow();
   assert.match(graph(), /<button[^>]*onclick="openMapGlobalFallback\(\)"[^>]*><strong>全局设备兜底<\/strong>/);
 }
 const edits=[];
 c.chooseDefaultGroup=kind=>edits.push(kind);
 c.openMapGlobalFallback();
 assert.deepEqual(edits,['global'],'the fallback editor must always target global settings');
 console.log('PASS: unnamed destination rule and global fallback have separate outlets and exclusive trace paths');
}
