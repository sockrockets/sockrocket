// Synthetic nodes only: exercise the shipped group editor and device policy UI.
const fs=require('node:fs'), vm=require('node:vm'), assert=require('node:assert/strict');
const source=fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
const elements={}, requests=[];
const el=id=>elements[id]||=( {
  value:'',hidden:true,disabled:false,options:[],textContent:'',_html:'',
  get innerHTML(){return this._html;},set innerHTML(value){this._html=value;var checks=Array.from(value.matchAll(/<input type="(?:checkbox|radio)" value="([^"]*)"([^>]*)>[\s\S]*?<span>([\s\S]*?)<\/span>/g),m=>({value:m[1],selected:/\bchecked\b/.test(m[2]),textContent:m[3]}));this.options=checks.length?checks:Array.from(value.matchAll(/<option value="([^"]*)"([^>]*)>(.*?)<\/option>/g),m=>({value:m[1],selected:m[2].includes('selected'),textContent:m[3]}));if(this.options.length)this.value=this.options[0].value;},
  appendChild(o){this.options.push(o);}
});
const c=vm.createContext({srElement:el,srEsc:v=>String(v??''),srApi:(...args)=>requests.push(args),srShowMsg(){},toastResp(){},invalidateRuleTrace(){},confirm:()=>true,document:{createElement:()=>({})},RULE_TARGET_LABEL:{proxy:'代理',direct:'直连',reject:'拒绝'},SR_PAGE_LOADED:{},SR_STATE:{ruleRows:[]},RULE_RULE_REVISION:'r1',isFinalRule:r=>['final','match'].includes(r.rule_type),ruleApi:(...args)=>requests.push(args)});
vm.runInContext(source.slice(source.indexOf('var SR_PROXY_GROUPS='),source.indexOf('/* ══ Routing rules')),c);
const take=action=>{const i=requests.findIndex(r=>r[0]===action);assert.ok(i>=0,action);return requests.splice(i,1)[0];};
const nodes=[{name:'Example A',fingerprint:'a'.repeat(64)},{name:'Example B',fingerprint:'b'.repeat(64)}];
const group={name:'Video',type:'select',members:nodes.map(n=>n.fingerprint),current:nodes[1].fingerprint,selected_node:'Example B'};
c.loadProxyGroups();take('proxy_groups')[2]({ok:true,revision:'r1',groups:[group],nodes});
assert.equal(el('rule-target').options.at(-1).value,'group:Video');
c.editProxyGroup(0);
assert.deepEqual(c.proxyGroupMemberState().filter(row=>row.selected).map(row=>row.value),[nodes[1].fingerprint],'manual mode keeps only the chosen node');
assert.equal(el('pg-member-tools').hidden,true,'manual mode has no select-all');
c.setProxyGroupMember(nodes[0].fingerprint,true);
assert.deepEqual(c.proxyGroupMemberState().filter(row=>row.selected).map(row=>row.value),[nodes[0].fingerprint],'choosing another node replaces the previous one');
el('pg-name').value='Media';c.saveProxyGroup();let req=take('save_proxy_group');
assert.equal(req[1].original_name,'Video');assert.equal(req[1].revision,'r1');
assert.deepEqual(req[1].group.members,[nodes[0].fingerprint],'manual save keeps the single node');
assert.equal(req[1].group.current,nodes[0].fingerprint);
c.saveProxyGroup();assert.equal(requests.length,0,'double save is blocked');
req[2]({ok:false,msg:'stale'});assert.equal(el('proxy-group-editor').hidden,false,'failed saves retain draft');
el('pg-type').value='fallback';c.updateProxyGroupMode();
assert.equal(el('pg-member-tools').hidden,false);
c.setProxyGroupMembers(true);
assert.equal(c.proxyGroupMemberState().filter(row=>row.selected).length,2,'fallback can include every node');
assert.equal(c.proxyGroupCurrent(),nodes[0].fingerprint,'fallback keeps the node chosen in manual mode');
el('pg-type').value='select';c.updateProxyGroupMode();
assert.deepEqual(c.proxyGroupMemberState().filter(row=>row.selected).map(row=>row.value),[nodes[0].fingerprint]);
take('proxy_groups')[2]({ok:true,revision:'r2',groups:[group],nodes});
assert.equal(c.PG_EDIT_REVISION,'r1','fresh catalog cannot silently rebase an old editor');
assert.equal(el('pg-name').value,'Media');
c.loadProxyGroups();const old=take('proxy_groups');c.loadProxyGroups();take('proxy_groups')[2]({ok:true,revision:'new',groups:[group],nodes});old[2]({ok:true,revision:'old',groups:[],nodes:[]});assert.equal(c.PG_REVISION,'new');
// Load the real source/CIDR, phase and locking helpers used by the device editor.
vm.runInContext(source.slice(source.indexOf('var RULE_TYPE_LABEL'),source.indexOf('var ruleExportPending')),c);
c.RULE_RULE_REVISION='r1';
el('device-available-groups').querySelectorAll=()=>el('device-available-groups').options.filter(box=>box.checked);
c.ruleApi=(...args)=>requests.push(args);
c.SR_STATE.devices=[{source:'192.0.2.8/32',key:'mac:00:11:22:33:44:55',alias_key:'mac:00:11:22:33:44:55',alias:'电视',host:'living-room'}];
c.SR_STATE.ruleRows=[
  {index:0,rule_type:'final',source:'192.0.2.8',target:'direct'},
  {index:1,rule_type:'domain',source:'',group:'Emby',enabled:true,pattern:'media.invalid',target:'proxy'},
];
c.SR_STATE.deviceBindings=[{source:'192.0.2.8/32',group:'Emby',target:'direct'}];
c.renderDevicePolicies();
assert.match(el('device-picker').innerHTML,/电视/);
assert.equal(el('device-picker').options.filter(o=>o.value.startsWith('192.0.2.8')).length,1,'a bare IP and its host CIDR identify the same device');
el('device-picker').value='192.0.2.8/32';c.selectPolicyDevice();
assert.equal(el('device-target').value,'direct');
assert.equal(el('device-group').value,'Emby');
assert.match(el('global-rule-summary').textContent,/共 2 条，已启用 2 条/);
assert.equal(el('device-group-target').value,'direct');
assert.equal(el('device-unlink').hidden,false);
for(const name of ['Music','Work'])c.SR_STATE.ruleRows.push({index:c.SR_STATE.ruleRows.length,rule_type:'domain',source:'',group:name,enabled:true,pattern:name+'.invalid',target:'proxy'});
c.updateDevicePolicySelection();
const choices=el('device-available-groups').options;
assert.deepEqual(choices.map(row=>row.value),['Music','Work']);
choices.forEach(box=>box.checked=true);c.updateDeviceAddCount();
assert.equal(el('device-add-selected').disabled,false);
el('device-add-priority').value='8';c.addDeviceGroups();const batch=take('save_device_binding')[1];
assert.equal(batch.priority,8);assert.deepEqual([...batch.groups],['Music','Work']);assert.equal(batch.source,'192.0.2.8/32');
assert.equal(requests.length,0,'adding multiple groups sends a single request');
const stages=c.ruleFlowModel(c.SR_STATE.ruleRows,'rule','192.0.2.8');
assert.deepEqual([...stages.find(s=>s.kind==='device_link').targets],['direct']);
el('device-group-priority').value='15';el('device-group-target').value='group:Video';c.saveDeviceBinding(false);req=take('save_device_binding');
assert.equal(req[1].priority,15);assert.equal(req[1].source,'192.0.2.8/32');assert.equal(req[1].target,'group:Video');assert.equal(req[1].group,'Emby');
c.saveDeviceBinding(true);assert.equal(take('save_device_binding')[1].remove,true);
el('device-alias').value='客厅电视';c.saveDeviceAlias();req=take('save_device_alias');
assert.equal(req[1].previous,'电视');assert.equal(req[1].name,'客厅电视');assert.equal(req[1].key,c.SR_STATE.devices[0].key);
el('device-picker').value='manual';c.selectPolicyDevice();c.saveDevicePolicy();
assert.equal(requests.length,0,'an empty manual source can never change the global default');
el('device-source').value='2001:db8:1::/64';c.updateDevicePolicySelection();
assert.equal(el('device-target').value,'inherit','switching devices must not carry over the other device exit');
el('device-target').value='group:Video';c.saveDevicePolicy();req=take('save_device_policy');
assert.equal(req[1].source,'2001:db8:1::/64');assert.equal(req[1].target,'group:Video');
el('device-picker').value='global';c.selectPolicyDevice();c.saveDevicePolicy();req=take('save_device_policy');
assert.equal(req[1].source,'');assert.equal(req[1].target,'inherit');
console.log('PASS: group selection, named router clients, device bindings, graph, manual CIDR and explicit global policy');

el('device-picker').value='global';c.selectPolicyDevice();
assert.equal(el('device-specific').hidden,true,'device naming is only available for a concrete device');
el('device-available-groups').options.find(box=>box.value==='Music').checked=true;
c.addDeviceGroups();req=take('save_device_binding');
assert.equal(req[1].source,'');assert.equal(req[1].all_devices,true);assert.deepEqual([...req[1].groups],['Music']);
c.SR_STATE.deviceBindings.push({source:'',group:'Music',target:'direct',priority:8});
c.updateDevicePolicySelection();assert.equal(el('device-group-priority').value,'8');
c.saveDeviceBinding(true);req=take('save_device_binding');
assert.equal(req[1].all_devices,true);assert.equal(req[1].group,'Music');assert.equal(req[1].remove,true);
