// Exercise the shipped workspace with interleaved groups and asynchronous writes.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync('merlin/webui/sockrocket.asp','utf8');
const elements={},requests=[],messages=[];let allowDiscard=true,confirmations=0;
function element(id){return elements[id]||={value:'',checked:true,hidden:false,disabled:false,style:{},parentElement:{style:{}},innerHTML:'',textContent:'',setAttribute(){},focus(){},scrollIntoView(){},showModal(){this.open=true;},close(){this.open=false;}};}
const c=vm.createContext({confirm:()=>{confirmations++;return allowDiscard;},SR_PROXY_GROUPS:[],setProxyGroupTargets(){},renderDevicePolicies(){},srElement:element,srApi:(...args)=>requests.push(args),srShowMsg:m=>messages.push(m),toastResp(){},srEsc:v=>String(v??'').replace(/[&<>"']/g,'_'),SR_STATE:{status:{running:false,proxy_mode:'rule'},ruleRows:[]}});
vm.runInContext(source.slice(source.indexOf('var RULE_TYPE_LABEL'),source.indexOf('var ruleExportPending')),c);
const rule=(index,group,type,pattern,extra={})=>({index,group,rule_type:type,pattern,target:'proxy',source:'',priority:0,enabled:true,...extra});
let rows=[rule(0,'Services','domain-suffix','example.invalid',{priority:30}),rule(1,'Other','domain','other.invalid',{priority:20}),rule(2,'Services','ip-cidr','192.0.2.0/24'),rule(3,'Services','dst-port','443',{source:'192.0.2.4',target:'direct'}),rule(4,'Services','final','*',{enabled:false}),rule(5,'','geoip','CN')];
c.SR_STATE.ruleRows=rows;c.RULE_RULE_REVISION='v1';c.renderRules();
assert.equal(c.RULE_SELECTED_GROUP,'Services');assert.match(element('rule-count').textContent,/本组共 4 条/);
for(const i of [0,2,3,4])assert.match(element('rule-list').innerHTML,new RegExp('editRule\\('+i+'\\)'));
assert.doesNotMatch(element('rule-list').innerHTML,/other.invalid/);
assert.equal(c.changeRuleGroup('Services','global'),true);
assert.doesNotMatch(element('rule-list').innerHTML,/editRule\(3\)/,'a mixed group can show only the rules shared by all devices');
assert.match(element('rule-list').innerHTML,/editRule\(0\)/);
assert.equal(c.changeRuleGroup('Services'),true);
assert.match(element('rule-list').innerHTML,/editRule\(3\)/,'normal group navigation restores all sources');

vm.runInContext(source.slice(source.indexOf('function keepRulePickerPosition('),source.indexOf('function renderDevicePolicies()')),c);
element('rule-search').value='SERVICES';c.renderRules();assert.match(element('rule-count').textContent,/当前显示 4 条/);
element('rule-search').value='192.0.2.0';c.renderRules();assert.match(element('rule-count').textContent,/当前显示 1 条/);assert.match(element('rule-group-summary').textContent,/共 4 条/);
c.changeRuleWorkspaceMode('bulk');assert.equal(c.RULE_EDIT_MEMBERS.length,4);assert.equal(element('rule-list-panel').hidden,true);
assert.match(element('rule-batch').value,/DOMAIN-SUFFIX,example.invalid,PROXY/);assert.match(element('rule-batch').value,/SOURCE,192.0.2.4,DST-PORT,443,DIRECT/);assert.match(element('rule-batch').value,/MATCH,PROXY/);
assert.equal(c.ruleWorkspaceDirty(),false);element('rule-batch').value+='\nDOMAIN,new.invalid,PROXY';allowDiscard=false;
assert.equal(c.changeRuleGroup('Other'),false);assert.equal(c.RULE_SELECTED_GROUP,'Services');c.changeRuleWorkspaceMode('list');assert.equal(c.RULE_WORKSPACE_MODE,'bulk');
c.previewRuleChanges();element('rule-batch').value+='\nDOMAIN,late.invalid,PROXY';c.applyRulePreview();assert.equal(requests.length,0,'a changed preview cannot be applied');
c.previewRuleChanges();c.applyRulePreview();let req=requests.shift();assert.equal(req[0],'replace_rule_group');assert.equal(req[1].expected_rules.length,4);assert.equal(req[1].priority,undefined,'preserve each original priority');assert.equal(req[1].expected_rules[3].enabled,false);
req[2]({ok:false,msg:'conflict'});requests.shift()[2]({ok:true,rules:rows,revision:'v2'});assert.equal(c.ruleWorkspaceDirty(),true,'failed save preserves draft');assert.equal(c.rulePending,false);
allowDiscard=true;c.changeRuleGroup('Other');assert.equal(c.RULE_WORKSPACE_MODE,'list');assert.equal(element('rule-search').value,'');
c.startRuleAddition();element('rule-type').value='domain-keyword';element('rule-pattern').value='media';c.addRule();req=requests.shift();assert.equal(req[0],'add_rule');assert.equal(req[1].group,'Other');
req[2]({ok:true});rows=rows.concat(rule(6,'Other','domain-keyword','media'));requests.shift()[2]({ok:true,rules:rows,revision:'v3'});assert.equal(c.RULE_SELECTED_GROUP,'Other');assert.equal(c.ruleGroupMembers('Other').length,2);assert.equal(c.ruleWorkspaceDirty(),false);
c.editRule(2);assert.equal(c.RULE_SELECTED_GROUP,'Services');assert.equal(element('rule-edit-panel').hidden,false);element('edit-rule-pattern').value='198.51.100.0/24';c.saveRuleEdit();req=requests.shift();assert.equal(req[0],'update_rule');assert.equal(req[1].index,2);assert.equal(req[1].expected.pattern,'192.0.2.0/24');
req[2]({ok:true});rows=rows.map(r=>r.index===2?{...r,pattern:'198.51.100.0/24'}:r);requests.shift()[2]({ok:true,rules:rows,revision:'v4'});assert.equal(element('rule-edit-panel').hidden,true);
c.changeRuleGroup('');c.startRuleAddition();element('rule-pattern').value='ungrouped.invalid';c.addRule();req=requests.shift();assert.equal(req[1].group,'','legacy unnamed group supports append');req[3]({});requests.shift()[2]({ok:true,rules:rows,revision:'v5'});assert.equal(element('rule-pattern').value,'ungrouped.invalid');
allowDiscard=false;c.selectRuleFlow(0);assert.equal(c.RULE_SELECTED_GROUP,'','diagram selection protects drafts');allowDiscard=true;c.cancelRuleEditor();
element('rule-new-group-name').value='New';c.createRuleGroup();assert.equal(c.RULE_SELECTED_GROUP,'New');assert.equal(c.ruleGroupMembers('New').length,0);assert.equal(element('rule-add-panel').hidden,false);
c.changeRuleWorkspaceMode('bulk');assert.equal(c.RULE_EDIT_GROUP,null);assert.equal(element('rule-entry-mode').value,'bulk');element('rule-batch').value='GEOIP,JP,PROXY\nDOMAIN,new.invalid,DIRECT';c.addRule();req=requests.shift();assert.equal(req[0],'batch_add_rules');assert.equal(req[1].group,'New');req[2]({ok:true});rows=rows.concat(rule(7,'New','geoip','JP'),rule(8,'New','domain','new.invalid',{target:'direct'}));requests.shift()[2]({ok:true,rules:rows,revision:'v6'});assert.equal(c.RULE_LOCAL_GROUPS.includes('New'),false,'persisted groups leave ephemeral state');
c.cancelRuleEditor();c.SR_STATE.ruleRows=rows.filter(r=>r.group!=='New');c.renderRules();assert.notEqual(c.RULE_SELECTED_GROUP,'New','deletion cannot resurrect an ephemeral group');
assert.equal(requests.length,0);
console.log('PASS: complete mixed groups, append, inline editing, bulk concurrency, draft protection, empty/unnamed groups and failure recovery');

// Viewing the current group must behave identically through every entry point.
c.changeRuleGroup('Services');
c.RULE_PRESETS=[{id:'services',group:'Services',target:'proxy',domains:[]}];
element('rule-preset').value='services';
const serviceFlow=c.ruleFlowModel(c.SR_STATE.ruleRows,'rule','').findIndex(r=>r.indices && r.indices.includes(0));
for(const [name,view] of [
  ['picker',()=>c.changeRuleGroup('Services')],
  ['diagram',()=>c.selectRuleFlow(serviceFlow)],
  ['diagram details',()=>c.showFlowRuleList('global_rule',serviceFlow)],
  ['preset',()=>c.viewRulePreset()]
]){
  c.changeRuleWorkspaceMode('bulk');element('rule-batch').value+='\nDOMAIN,draft.invalid,PROXY';
  element('rule-search').value='no-match';element('rule-filter-target').value='reject';element('rule-filter-state').value='disabled';c.renderRules();
  const draft=element('rule-batch').value;
  allowDiscard=false;let before=confirmations;view();
  assert.equal(confirmations,before+1,name+' asks before replacing a same-group draft');
  assert.equal(c.RULE_WORKSPACE_MODE,'bulk');assert.equal(element('rule-batch').value,draft);
  assert.equal(element('rule-search').value,'no-match');assert.equal(c.RULE_SELECTED_GROUP,'Services');
  allowDiscard=true;before=confirmations;view();
  assert.equal(confirmations,before+1,name+' prompts only once');
  assert.equal(c.RULE_WORKSPACE_MODE,'list');assert.equal(element('rule-list-panel').hidden,false);
  assert.equal(element('rule-add-panel').hidden,true);assert.equal(element('rule-edit-panel').hidden,true);
  for(const id of ['rule-search','rule-filter-target','rule-filter-state'])assert.equal(element(id).value,'',name+' clears '+id);
  assert.match(element('rule-count').textContent,/当前显示 4 条/);
}
// Cancellation also protects an inline row draft in the same group.
c.editRule(2);element('edit-rule-pattern').value='203.0.113.0/24';allowDiscard=false;c.viewRulePreset();
assert.equal(element('rule-edit-panel').hidden,false);assert.equal(element('edit-rule-pattern').value,'203.0.113.0/24');
allowDiscard=true;c.viewRulePreset();assert.equal(element('rule-edit-panel').hidden,true);
console.log('PASS: same-group navigation clears filters, restores the list and protects drafts through all entry points');

// Reproduce native HTMLSelectElement semantics: assigning a missing option yields "".
// The original generic value mock accepted missing values and hid this regression.
const target=element('rule-target');let targetValue='proxy';
let targetOptions=['proxy','direct','reject'];
Object.defineProperty(target,'value',{configurable:true,get:()=>targetValue,set:value=>{targetValue=targetOptions.includes(String(value))?String(value):'';}});
const missing='node:'+'a'.repeat(64),known='node:'+'b'.repeat(64);
c.SR_STATE.ruleRows=[rule(0,'Stale','domain','stale.invalid',{target:missing})];
c.renderRules();c.changeRuleWorkspaceMode('bulk');
assert.equal(target.value,'proxy','missing defaults have a valid fallback');
assert.equal(element('rule-default-fields').hidden,false,'bulk default action must be visible and editable');
assert.equal(element('rule-target').parentElement.hidden,false);
assert.match(element('rule-batch').value,new RegExp(missing),'opening the editor must not silently rewrite explicit actions');
element('rule-batch').value='DOMAIN,stale.invalid,DIRECT';target.value='direct';c.previewRuleChanges();
assert.match(element('rule-preview-summary').textContent,/未填写 ACTION 的行默认处理：直连/);c.applyRulePreview();req=requests.shift();
assert.equal(req[0],'replace_rule_group');assert.equal(req[1].target,'direct');assert.equal(req[1].text,'DOMAIN,stale.invalid,DIRECT');
assert.equal(req[1].expected_rules[0].target,missing,'concurrency snapshot keeps the original target');
req[2]({ok:true});requests.shift()[2]({ok:true,rules:[rule(0,'Stale','domain','stale.invalid',{target:'direct'})],revision:'v7'});
targetOptions.push(known);c.RULE_TARGET_LABEL[known]='Example node';
c.SR_STATE.ruleRows=[rule(0,'Known','domain','known.invalid',{target:known})];c.renderRules();c.changeRuleWorkspaceMode('bulk');
assert.equal(target.value,known,'valid group defaults remain selected');assert.match(element('rule-batch').value,new RegExp(known));
c.cancelRuleEditor();assert.equal(requests.length,0);
console.log('PASS: native select missing-option semantics, editable bulk defaults, corrected requests and preserved explicit actions');

// Keep the active picker anchored when a different group's list changes height.
{
  let offset = 450, scrolled = 0;
  const picker = {getBoundingClientRect: () => ({top: offset})};
  c.window = {scrollBy(x, y) { assert.equal(x, 0); scrolled += y; offset -= y; }};
  c.keepRulePickerPosition(picker, () => { offset -= 180; });
  assert.equal(offset, 450);
  assert.equal(scrolled, -180);
}

allowDiscard=true;c.rulePending=false;c.RULE_READ_PENDING=false;c.RULE_FLOW_ERROR=false;c.RULE_RULE_REVISION='switch';c.SR_STATE.ruleRows=[rule(2,'Services','domain','example.invalid')];c.changeRuleGroup('Services');c.startRuleAddition();c.editRule(2);
assert.equal(element('rule-add-panel').hidden,true,'editing an existing rule closes addition');
assert.equal(element('rule-edit-panel').hidden,false);
