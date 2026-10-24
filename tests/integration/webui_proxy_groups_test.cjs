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
el('device-source').value='2001:db8:1::/64';el('device-target').value='group:Video';c.saveDevicePolicy();req=take('save_device_policy');assert.equal(req[1].source,'2001:db8:1::/64');assert.equal(req[1].target,'group:Video');
c.SR_STATE.ruleRows=[{index:4,rule_type:'final',source:'192.0.2.8',target:'direct'}];
el('device-target').value='group:Video';el('device-source').value='192.0.2.9';c.updateDevicePolicySelection();
assert.equal(el('device-target').value,'group:Video','typing a new source keeps the chosen exit');
el('device-source').value='192.0.2.8';c.updateDevicePolicySelection();
assert.equal(el('device-target').value,'direct','an exact saved source loads its policy');
c.SR_STATE.ruleRows=[];
el('device-source').value='';el('device-target').value='inherit';c.saveDevicePolicy();req=take('save_device_policy');assert.equal(req[1].source,'');assert.equal(req[1].target,'inherit');
console.log('PASS: group member selection, target menus, duplicate-save guard, stale editor/read protection and default/device inheritance');
