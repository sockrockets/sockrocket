<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Pragma" content="no-cache">
<meta http-equiv="Expires" content="-1">
<link rel="shortcut icon" href="/images/favicon.png">
<title>Sockrocket</title>
<!-- Written by sockrocket.sh (api-start): defines SOCKROCKET_API_PORT from config.yaml's
     api_port. Missing on koolcenter / old installs — the code falls back. -->
<script src="api.js"></script>
<link rel="stylesheet" href="/index_style.css">
<link rel="stylesheet" href="/form_style.css">
<script src="/js/jquery.js"></script>
<script src="/state.js"></script>
<script src="/general.js"></script>
<script src="/popup.js"></script>
<script src="/help.js"></script>
<style>
/* The firmware owns its header and menu. Every plugin rule is scoped here. */
#sockrocket-plugin {
  --bg-app:#eef1f4;--bg-sidebar:#fff;--bg-hover:#f0f3f6;--border:#dfe5eb;
  --text-primary:#202733;--text-muted:#667380;--text-secondary:#515e6c;
  --accent:#087ff5;--accent-dim:#e9f3ff;--text-accent:#0066ce;
  --success:#19815e;--warning:#a56800;--danger:#bd3434;
  --sans:-apple-system,BlinkMacSystemFont,"PingFang SC","Microsoft YaHei",sans-serif;
  --mono:"SF Mono",Consolas,Menlo,monospace;
  color:var(--text-primary);background:var(--bg-app);font:13px/1.6 var(--sans);
  min-height:680px;border-radius:12px;isolation:isolate;text-align:left;
}
#sockrocket-plugin *,#sockrocket-plugin *::before,#sockrocket-plugin *::after {box-sizing:border-box}
#sockrocket-plugin :is(h1,h2,h3,p,dl,dd) {margin:0}
#sockrocket-plugin [hidden] {display:none!important}
#sockrocket-plugin button,#sockrocket-plugin input,#sockrocket-plugin select,#sockrocket-plugin textarea {font:inherit;color:inherit}
#sockrocket-plugin .rule-preview {white-space:pre-wrap;overflow-wrap:anywhere;max-height:320px;overflow:auto;background:var(--bg-hover);padding:14px;border-radius:8px;font:12px/1.8 var(--mono)}
#sockrocket-plugin button {cursor:pointer}
#sockrocket-plugin button:disabled {opacity:.5;cursor:default}
#sockrocket-plugin :is(button,input,select,textarea,[tabindex]):focus-visible {outline:3px solid #087ff555;outline-offset:3px}
#sockrocket-plugin #sr-app {padding:28px;min-width:0}
#sockrocket-plugin .breadcrumb {font-size:11px;color:var(--text-muted);margin-bottom:16px}
#sockrocket-plugin #sr-titlebar {display:flex;align-items:center;flex-wrap:wrap;gap:14px;margin-bottom:20px;min-width:0}
#sockrocket-plugin .logo {display:flex;align-items:center;gap:8px;font-size:24px;font-weight:650;letter-spacing:-.5px}
#sockrocket-plugin .logo svg {color:var(--accent);flex-shrink:0}
#sockrocket-plugin .project-links {display:flex;flex-wrap:wrap;gap:6px 16px;min-width:0}
#sockrocket-plugin .project-links a {color:var(--text-accent);font-weight:600;text-decoration:underline;overflow-wrap:anywhere}
#sockrocket-plugin .spacer {flex:1}
#sockrocket-plugin .service-pill {font-size:12px;color:var(--text-muted);background:#e4e8ed;padding:4px 13px;border-radius:7px;white-space:nowrap}
#sockrocket-plugin .service-pill.success {color:var(--success);background:#e4f5ee}
#sockrocket-plugin .node-badge {display:flex;gap:6px;align-items:center;min-width:0;max-width:260px;border:1px solid var(--border);border-radius:7px;background:var(--bg-sidebar);padding:6px 10px;font-size:11px}
#sockrocket-plugin .nb-name {overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
#sockrocket-plugin .nb-lat {color:var(--accent);flex-shrink:0}
#sockrocket-plugin .nb-lat.bad {color:var(--danger)}
#sockrocket-plugin #sr-main {min-width:0}
#sockrocket-plugin #sr-sidebar {display:flex;gap:4px;overflow-x:auto;white-space:nowrap;background:var(--bg-sidebar);border:1px solid var(--border);border-radius:9px;padding:4px;margin-bottom:22px}
#sockrocket-plugin .nav-btn {border:0;background:transparent;color:var(--text-muted);display:flex;gap:6px;align-items:center;justify-content:center;flex:1 0 auto;padding:9px 12px;border-radius:6px;min-height:38px}
#sockrocket-plugin .nav-btn svg {width:16px;height:16px;flex-shrink:0}
#sockrocket-plugin .nav-btn.active {color:var(--accent);background:var(--accent-dim);font-weight:600}
#sockrocket-plugin #sr-content {min-width:0}
#sockrocket-plugin .page {display:none}
#sockrocket-plugin .page.active {display:block}
#sockrocket-plugin .page-title {font-size:20px;font-weight:600;margin-bottom:8px}
#sockrocket-plugin .page-intro {color:var(--text-muted);margin-bottom:18px}
#sockrocket-plugin .card,#sockrocket-plugin .stat {background:var(--bg-sidebar);border:1px solid var(--border);border-radius:12px;padding:20px;margin-bottom:16px}
#sockrocket-plugin .card h3,#sockrocket-plugin .section-heading {font-size:15px;font-weight:600;margin-bottom:16px}
#sockrocket-plugin .hero {display:flex;align-items:center;gap:16px;min-width:0}
#sockrocket-plugin .hero-icon {width:44px;height:44px;display:flex;align-items:center;justify-content:center;border-radius:12px;background:var(--accent-dim);color:var(--accent);flex-shrink:0}
#sockrocket-plugin .hero-icon.off {background:var(--bg-hover);color:var(--text-muted)}
#sockrocket-plugin .hero-icon svg {width:20px;height:20px}
#sockrocket-plugin .hero-info {flex:1;min-width:0}
#sockrocket-plugin .hero-title {display:flex;gap:8px;align-items:center;flex-wrap:wrap;font-size:20px;font-weight:600}
#sockrocket-plugin .hero-title .live {display:none!important}
#sockrocket-plugin .hero-sub {color:var(--text-muted);font-size:12px;overflow-wrap:anywhere;margin-top:6px}
#sockrocket-plugin .hero-metrics {display:flex;gap:14px;flex-wrap:wrap;color:var(--text-muted);font-size:12px;margin-top:8px}
#sockrocket-plugin .hero-metrics b {font-weight:500;color:var(--text-primary)}
#sockrocket-plugin .network-summary {display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px;margin:18px 0}
#sockrocket-plugin .network-summary>span {background:var(--bg-sidebar);border:1px solid var(--border);border-radius:12px;padding:16px 20px;display:flex;flex-direction:column;gap:12px;font-weight:600}
#sockrocket-plugin .network-summary b {font-weight:400;color:var(--text-muted)}
#sockrocket-plugin #sr-status-freshness {grid-column:1/-1;padding:0;background:none;border:0;font-size:11px;font-weight:400;color:var(--text-muted)}
#sockrocket-plugin .stat-grid {display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px;margin:18px 0}
#sockrocket-plugin .stat {margin:0;padding:18px;min-width:0}
#sockrocket-plugin .stat .label {font-size:12px;color:var(--text-muted)}
#sockrocket-plugin .stat .value {font-size:25px;font-weight:600;margin-top:6px;overflow-wrap:anywhere}
#sockrocket-plugin .stat .sub {font-size:11px;color:var(--text-muted);overflow-wrap:anywhere;margin-top:4px}
#sockrocket-plugin .ok,#sockrocket-plugin .check-ok,#sockrocket-plugin .good {color:var(--success)}
#sockrocket-plugin .bad,#sockrocket-plugin .check-bad {color:var(--danger)}
#sockrocket-plugin .acc {color:var(--accent)}
#sockrocket-plugin .mid {color:var(--warning)}
#sockrocket-plugin .toolbar {display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:14px}
#sockrocket-plugin .btn {display:inline-flex;align-items:center;justify-content:center;gap:6px;min-height:36px;padding:7px 14px;border:1px solid transparent;border-radius:7px;background:#edf1f5;color:var(--text-primary);font-size:12px;text-decoration:none;white-space:nowrap}
#sockrocket-plugin .btn:hover {filter:brightness(.97)}
#sockrocket-plugin .btn-accent,#sockrocket-plugin .btn-success {background:var(--accent);color:#fff}
#sockrocket-plugin .btn-danger {background:#fff0ef;color:var(--danger);border-color:#f2d8d5}
#sockrocket-plugin .btn-sm {min-height:32px;padding:5px 10px}
#sockrocket-plugin .chip {display:inline-flex;align-items:center;gap:6px;min-height:36px;padding:7px 10px;border:1px solid var(--border);border-radius:7px;background:var(--bg-sidebar);font-size:12px}
#sockrocket-plugin .chip b {font-weight:500}
#sockrocket-plugin .dot {width:6px;height:6px;border-radius:50%;background:var(--text-muted)}
#sockrocket-plugin .dot.ok {background:var(--success)}
#sockrocket-plugin .dot.acc {background:var(--accent)}
#sockrocket-plugin .pending {opacity:.6;pointer-events:none}
#sockrocket-plugin .mode-seg {display:inline-flex;gap:3px;background:#e3e8ed;padding:3px;border-radius:8px}
#sockrocket-plugin .mode-seg button {border:0;border-radius:6px;background:none;color:var(--text-muted);padding:7px 13px;min-height:30px}
#sockrocket-plugin .mode-seg .on {background:white;color:var(--accent);font-weight:600;box-shadow:0 1px 3px #0001}
#sockrocket-plugin .rows {display:flex;flex-direction:column;min-width:0}
#sockrocket-plugin .row {display:flex;gap:10px;align-items:center;min-height:62px;padding:12px 8px;border-bottom:1px solid #edf0f3;min-width:0}
#sockrocket-plugin .row:last-child {border-bottom:0}
#sockrocket-plugin .row.active {background:var(--accent-dim);border-radius:8px}
#sockrocket-plugin .r-name {flex:1;min-width:0;overflow-wrap:anywhere}
#sockrocket-plugin .r-name .r-sub {display:block;margin-top:4px}
#sockrocket-plugin .r-sub {font-size:11px;color:var(--text-muted)}
#sockrocket-plugin .node-inspect {text-align:left;border:0;background:transparent;padding:4px;color:inherit;font:inherit}
#sockrocket-plugin .badge {font-size:10px;padding:3px 7px;border-radius:5px;background:var(--bg-hover);flex-shrink:0}
#sockrocket-plugin .badge.acc {background:var(--accent-dim)}
#sockrocket-plugin .lat {font-size:12px;text-align:right;min-width:56px;flex-shrink:0}
#sockrocket-plugin .hint {font-size:12px;line-height:1.7;color:var(--text-muted);margin-top:8px;overflow-wrap:anywhere}
#sockrocket-plugin .form-grid {display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px;margin-bottom:14px}
#sockrocket-plugin .fg {display:flex;flex-direction:column;gap:5px;min-width:0}
#sockrocket-plugin .fg label {font-size:12px;color:var(--text-muted)}
#sockrocket-plugin :is(input[type=text],input[type=password],input[type=url],input[type=number],input[type=search],select,textarea) {min-width:0;width:100%;border:1px solid var(--border);border-radius:7px;background:white;padding:9px 11px;min-height:38px}
#sockrocket-plugin input[type=checkbox] {accent-color:var(--accent)}
#sockrocket-plugin textarea {font:12px/1.7 var(--mono);resize:vertical;min-height:150px}
#sockrocket-plugin #sr-page-network textarea {min-height:76px;overflow-wrap:anywhere}
#sockrocket-plugin #sr-page-network .fg {margin-bottom:12px}
#sockrocket-plugin #sr-config-text {height:400px;margin-top:12px}
#sockrocket-plugin #sr-node-search {flex:1;min-width:150px;max-width:340px}
#sockrocket-plugin .config-state {font-size:12px;color:var(--text-muted);margin-top:12px}

#sockrocket-plugin .rule-workspace-controls {display:grid;grid-template-columns:minmax(0,1.4fr) minmax(0,1fr) minmax(0,1.4fr);gap:12px;margin:14px 0 10px}
#sockrocket-plugin .rule-workspace-controls .fg {min-width:0}
#sockrocket-plugin .rule-workspace-controls select {width:100%;min-width:0}
#sockrocket-plugin .rule-workspace-actions {margin:12px 0}
#sockrocket-plugin .rule-inline-editor {padding:16px;background:var(--bg-hover);border:1px solid var(--border);border-radius:9px;margin:14px 0}
#sockrocket-plugin .rule-inline-editor .toolbar {margin-bottom:12px}
#sockrocket-plugin #sr-rule-list-panel {border-top:1px solid var(--border);margin-top:14px;padding-top:4px}
@media(max-width:700px) {#sockrocket-plugin .rule-workspace-controls {grid-template-columns:1fr 1fr} #sockrocket-plugin .rule-workspace-controls>.fg:first-child {grid-column:1/-1}}
#sockrocket-plugin .rule-filters {display:grid;grid-template-columns:minmax(0,1fr) 140px 120px;gap:8px;margin:12px 0}
#sockrocket-plugin .rule-main {flex:1;min-width:120px;overflow-wrap:anywhere}
#sockrocket-plugin .rule-pattern {font-family:var(--mono);overflow-wrap:anywhere}
#sockrocket-plugin .rule-meta {font-size:11px;color:var(--text-muted);margin-top:3px}
#sockrocket-plugin .rule-actions {display:flex;gap:5px;flex-wrap:wrap}
#sockrocket-plugin .rule-row {align-items:flex-start;flex-wrap:wrap}
#sockrocket-plugin .rule-phase {padding:10px 0;color:var(--text-muted);font-size:12px;border-top:1px dashed var(--border)}
@media(max-width:700px) {#sockrocket-plugin .rule-filters {grid-template-columns:1fr 1fr} #sockrocket-plugin .rule-filters input {grid-column:1/-1} #sockrocket-plugin .rule-actions {width:100%;justify-content:flex-end}}
#sockrocket-plugin .rule-group {border:1px solid var(--border);border-radius:9px;overflow:hidden;margin-bottom:12px}
#sockrocket-plugin .rule-group-hd {display:flex;align-items:center;flex-wrap:wrap;gap:8px;background:var(--bg-hover);padding:10px 12px;}
#sockrocket-plugin .rg-title {font-weight:600}
#sockrocket-plugin .rg-count {font-size:11px;color:var(--text-muted)}
#sockrocket-plugin .rg-actions {display:flex;gap:5px;flex-wrap:wrap;margin-left:auto}
#sockrocket-plugin .rule-group.collapsed .rule-group-body {display:none}
#sockrocket-plugin .rule-group-body {padding:0 8px}
#sockrocket-plugin .collapsible {display:none}
#sockrocket-plugin .collapsible.open {display:block}
#sockrocket-plugin .diag-row {display:flex;gap:14px;justify-content:space-between;align-items:center;padding:12px 0;border-bottom:1px solid var(--border)}
#sockrocket-plugin .diag-row:last-child {border:0}
#sockrocket-plugin .d-name {color:var(--text-secondary)}
#sockrocket-plugin #sr-log-view,#sockrocket-plugin .log-block {height:380px;max-width:100%;overflow:auto;border:1px solid var(--border);border-radius:9px;padding:16px;background:#f7f8fa;color:#343c48;font:12px/1.8 var(--mono);white-space:pre-wrap;overflow-wrap:anywhere}
#sockrocket-plugin #sr-statusbar {display:flex;align-items:center;gap:12px;flex-wrap:wrap;font-size:11px;color:var(--text-muted);border-top:1px solid var(--border);margin-top:24px;padding-top:12px}
#sockrocket-plugin .sb-left,#sockrocket-plugin .sb-right {display:flex;gap:8px;align-items:center;min-width:0;flex-wrap:wrap}
#sockrocket-plugin .sb-right {margin-left:auto}
#sockrocket-plugin .sb-left>span {min-width:0;overflow-wrap:anywhere}
#sockrocket-plugin .network-summary>span,#sockrocket-plugin .diag-row>span {min-width:0;overflow-wrap:anywhere}
#sockrocket-plugin .node-inspect {max-width:100%;overflow-wrap:anywhere}
#sockrocket-plugin #sr-msg {display:none;position:fixed;top:24px;right:24px;z-index:10000;max-width:min(420px,calc(100vw - 32px));border:1px solid var(--border);border-radius:10px;padding:14px 18px;background:white;box-shadow:0 8px 28px #0002;white-space:pre-wrap;overflow-wrap:anywhere}
#sockrocket-plugin #sr-msg.err {color:var(--danger);border-color:#f2d8d5}
#sockrocket-plugin #sr-msg.ok {color:var(--success)}
#sockrocket-plugin dialog {position:fixed;inset:0;margin:auto;border:1px solid var(--border);border-radius:14px;background:white;color:var(--text-primary);padding:26px;width:min(520px,calc(100vw - 32px));max-height:calc(100dvh - 40px);overflow:auto;box-shadow:0 20px 90px #0003}
#sockrocket-plugin dialog::backdrop {background:#20273366}
#sockrocket-plugin dialog h2 {font-size:22px;margin-bottom:14px}
#sockrocket-plugin dialog dl {display:grid;grid-template-columns:90px minmax(0,1fr);gap:14px;margin:22px 0}
#sockrocket-plugin dialog dt {color:var(--text-muted)}
#sockrocket-plugin dialog dd {overflow-wrap:anywhere}
#sockrocket-plugin .dialog-actions {display:flex;justify-content:flex-end;gap:8px;flex-wrap:wrap;margin-top:24px}

/* Keep the routing editor's frequent actions prominent and its row tools quieter. */
#sockrocket-plugin #sr-page-dns .toolbar > h3 {margin:0}
#sockrocket-plugin #sr-page-dns .card > h3 {margin-bottom:8px}
#sockrocket-plugin #sr-rule-workspace {container-type:inline-size;container-name:rule-editor}
#sockrocket-plugin #sr-rule-workspace-title {font-size:18px;line-height:1.4;letter-spacing:.01em}
#sockrocket-plugin #sr-rule-create-toggle {border:1px solid var(--border);background:white;color:var(--text-secondary)}
#sockrocket-plugin #sr-rule-workspace summary {cursor:pointer}
#sockrocket-plugin #sr-rule-workspace summary:focus-visible {outline:3px solid var(--text-accent);outline-offset:2px}
#sockrocket-plugin #sr-rule-workspace .rule-workspace-actions {
  display:flex;align-items:end;flex-wrap:wrap;gap:8px;margin:16px 0 4px;
}
#sockrocket-plugin .rule-workspace-actions > .fg {flex:1 1 12rem}
#sockrocket-plugin .rule-workspace-actions > details > summary {
  width:fit-content;border:1px solid var(--border);background:white;border-radius:7px;
  padding:8px 12px;min-height:38px;font-size:13px;font-weight:600;
}
#sockrocket-plugin .rule-workspace-actions > .rule-add-options > summary {background:var(--text-accent);color:white;border-color:var(--text-accent)}
#sockrocket-plugin .rule-workspace-actions > details[open] {flex:1 0 100%;min-width:0;padding:12px;border-radius:8px;background:var(--bg-hover)}
#sockrocket-plugin .rule-workspace-actions > details[open] > :not(summary) {margin-top:10px}
#sockrocket-plugin #sr-rule-list-panel {padding-top:14px}
#sockrocket-plugin #sr-rule-filter-panel > summary {width:fit-content;padding:5px 0;color:var(--text-secondary)}
#sockrocket-plugin #sr-rule-workspace .rule-filters {grid-template-columns:repeat(auto-fit,minmax(min(100%,9rem),1fr))}
#sockrocket-plugin #sr-rule-workspace .rule-phase {padding:12px 0 4px;color:var(--text-muted);font-size:12px;font-weight:600;border:0}
#sockrocket-plugin #sr-rule-workspace .rule-row {align-items:center;gap:10px;min-height:56px;padding:9px 0}
#sockrocket-plugin #sr-rule-workspace .rule-main {display:flex;align-items:center;flex-wrap:wrap;gap:3px 8px;min-width:0}
#sockrocket-plugin #sr-rule-workspace .rule-pattern {order:-1;width:100%;font-size:14px;font-weight:550;line-height:1.45}
#sockrocket-plugin #sr-rule-workspace .rule-main .badge {padding:0;border:0;background:transparent;color:var(--text-secondary);font-size:12px;max-width:100%;overflow-wrap:anywhere}
#sockrocket-plugin #sr-rule-workspace .rule-main .badge + .badge {border-left:1px solid var(--border);border-radius:0;padding-left:8px}
#sockrocket-plugin #sr-rule-workspace .rule-meta {margin:0}
#sockrocket-plugin #sr-rule-workspace .rule-actions {width:auto;max-width:100%;align-items:center;gap:6px}
#sockrocket-plugin #sr-rule-workspace .rule-actions > .btn {background:transparent;color:var(--text-accent);border-color:transparent}
#sockrocket-plugin #sr-rule-workspace .rule-actions > .btn:hover {background:var(--accent-dim)}
#sockrocket-plugin #sr-rule-workspace .rule-actions summary {color:var(--text-secondary);padding:6px}
#sockrocket-plugin #sr-rule-operation-state:empty {display:none}
@container rule-editor (max-width:32rem) {
  #sockrocket-plugin .rule-workspace-actions > .fg {flex-basis:100%}
  #sockrocket-plugin #sr-rule-workspace .rule-actions :is(button,summary),
  #sockrocket-plugin .rule-workspace-actions > details > summary {min-height:44px}
}

/* Configuration flow overview shares the firmware-integrated light palette. */
#sockrocket-plugin .rule-flow-head {display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:8px}
#sockrocket-plugin .rule-flow-head h3 {margin:0;flex:1}
#sockrocket-plugin .flow-lines {position:absolute;inset:0;width:100%;height:100%;pointer-events:none;overflow:visible}
#sockrocket-plugin .flow-lines path {fill:none;stroke:var(--border);stroke-width:1.5;vector-effect:non-scaling-stroke}
#sockrocket-plugin .flow-lines path.selected {stroke:var(--accent);stroke-width:2}
#sockrocket-plugin .flow-lines path.branch {stroke-dasharray:4 4}
#sockrocket-plugin .routing-map {position:relative;isolation:isolate;margin:8px 0 14px}
#sockrocket-plugin .routing-map .flow-lines {z-index:-1}
#sockrocket-plugin .map-head {display:grid;grid-template-columns:22% 34% 22%;justify-content:space-between;color:var(--text-muted);font-size:11px;margin-top:14px}
#sockrocket-plugin .map-node {position:absolute;box-sizing:border-box;display:flex;flex-direction:column;justify-content:center;gap:2px;height:54px;min-width:0;padding:5px 8px;border:1px solid var(--border);border-radius:8px;color:var(--text-primary);background:var(--bg-sidebar);font:inherit;font-size:12px;text-align:left}
#sockrocket-plugin button.map-node {cursor:pointer}
#sockrocket-plugin button.map-node:hover,#sockrocket-plugin .map-node.selected {border-color:var(--accent);background:var(--bg-hover)}
#sockrocket-plugin .map-node strong,#sockrocket-plugin .map-node small {overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
#sockrocket-plugin .map-node small {color:var(--text-muted);font-size:10px}
#sockrocket-plugin .map-node.source {left:0;width:22%}
#sockrocket-plugin .map-node.group {left:33%;width:34%}
#sockrocket-plugin .map-node.outlet {left:78%;width:22%}
#sockrocket-plugin .map-node.inactive {opacity:.55}
#sockrocket-plugin .routing-map path.related {stroke:var(--accent);opacity:.7}
#sockrocket-plugin .routing-map path.dim {opacity:.32}
#sockrocket-plugin #sr-device-settings > h4 {margin:14px 0 4px;font-size:14px}
#sockrocket-plugin #sr-device-bindings .row {min-height:48px}
#sockrocket-plugin #sr-device-form details > summary {padding:8px 0;cursor:pointer}
#sockrocket-plugin #sr-device-add-groups > summary {color:var(--text-accent);font-weight:600}
#sockrocket-plugin #sr-device-fallback {margin-top:12px;border-top:1px solid var(--border)}
#sockrocket-plugin .flow-query {display:grid;grid-template-columns:minmax(160px,2fr) minmax(120px,1fr) 85px;gap:10px;margin:16px 0 10px}
#sockrocket-plugin .flow-trace-result {margin-top:14px;border:1px solid var(--border);border-radius:9px;padding:14px;background:var(--bg-sidebar)}
#sockrocket-plugin .flow-trace-result h4 {margin:0;font-size:15px}
#sockrocket-plugin .flow-checks {max-height:300px;overflow:auto;margin:12px 0 0;padding-left:24px}
#sockrocket-plugin .flow-checks li {padding:7px 4px;border-bottom:1px solid var(--border);font-size:11px;overflow-wrap:anywhere}
#sockrocket-plugin .flow-checks .matched {color:var(--text-accent);font-weight:600}
#sockrocket-plugin .flow-warning {color:var(--warning);font-size:11px;margin-top:8px;overflow-wrap:anywhere}
#sockrocket-plugin .proxy-group-grid {grid-template-columns:repeat(2,minmax(0,1fr))}
#sockrocket-plugin #sr-rule-add-methods [aria-pressed="true"] {background:var(--accent);color:white}
#sockrocket-plugin .pg-member-list {max-height:220px;overflow:auto;border:1px solid var(--border);border-radius:8px;background:var(--bg-sidebar)}
#sockrocket-plugin .pg-member {display:flex;align-items:flex-start;gap:8px;margin:0;padding:7px 10px;border-bottom:1px solid var(--border);font-size:12px;line-height:1.4}
#sockrocket-plugin .pg-member:last-child {border-bottom:0}
#sockrocket-plugin .pg-member label {display:flex;align-items:flex-start;gap:8px;flex:1;min-width:0;margin:0;cursor:pointer}
#sockrocket-plugin .pg-member input {margin-top:2px;flex:none}
#sockrocket-plugin .pg-member-move {display:flex;gap:4px;flex:none}
#sockrocket-plugin .flow-detail {display:flex;gap:12px;align-items:center;flex-wrap:wrap;background:var(--bg-hover);border-radius:8px;padding:12px 14px;margin-top:16px;min-width:0}
#sockrocket-plugin .flow-detail>div {flex:1;min-width:140px;overflow-wrap:anywhere}
#sockrocket-plugin .flow-detail p {font-size:11px;color:var(--text-muted)}
#sockrocket-plugin .flow-state {font-size:11px;background:var(--bg-hover);color:var(--text-muted);border-radius:5px;padding:3px 8px}
#sockrocket-plugin .traffic-table {width:100%;border-collapse:collapse;font-size:12px}
#sockrocket-plugin .traffic-table th,#sockrocket-plugin .traffic-table td {padding:7px 8px;border-bottom:1px solid var(--border);text-align:right;white-space:nowrap}
#sockrocket-plugin .traffic-table th:first-child,#sockrocket-plugin .traffic-table td:first-child {text-align:left;max-width:240px;overflow:hidden;text-overflow:ellipsis}
#sockrocket-plugin .traffic-table tr:last-child td {border-bottom:0}
@media(max-width:600px) {
 #sockrocket-plugin .proxy-group-grid {grid-template-columns:1fr}
 #sockrocket-plugin .flow-query {grid-template-columns:1fr}
 #sockrocket-plugin .rule-flow-head h3 {flex-basis:100%}
}

body.bg {min-width:0!important}
#TopBanner,#footer {position:relative;width:998px;max-width:100%;margin-left:auto;margin-right:auto;overflow-x:auto}
#sr-firmware-layout {width:100%;max-width:998px;margin:0 auto;border-spacing:0;table-layout:fixed}
#sr-firmware-layout .sr-firmware-nav {width:202px;vertical-align:top}
#sr-firmware-layout .sr-firmware-content {vertical-align:top;padding:0 16px 24px}
#sr-host-notice {padding:12px 16px;background:#fff4d8;color:#795000;font:13px/1.5 sans-serif}
@media(max-width:1050px) {
 #sockrocket-plugin #sr-app {padding:20px}
 #sockrocket-plugin .row {flex-wrap:wrap}
 #sockrocket-plugin .r-name {flex-basis:180px}
 #sockrocket-plugin .adaptive-form {grid-template-columns:repeat(2,minmax(0,1fr))!important}
 #sockrocket-plugin .stat-grid {grid-template-columns:repeat(2,minmax(0,1fr))}
}
#sockrocket-plugin .router-home {display:none}
@media(max-width:998px) {
 #sr-firmware-layout .sr-firmware-nav {display:none}
 #sr-firmware-layout .sr-firmware-content {padding:0 8px 16px}
 #sockrocket-plugin .router-home {display:inline;margin-left:12px;color:var(--text-accent)}
}
@media(max-width:760px) {
 body.bg {min-width:0!important}
 #TopBanner,#footer {position:relative;max-width:100%;overflow-x:auto}
 #sockrocket-plugin .router-home {display:inline;margin-left:12px;color:var(--text-accent)}
 #sr-firmware-layout .sr-firmware-nav {display:none}
 #sr-firmware-layout .sr-firmware-content {padding:0 8px 16px}
 #sockrocket-plugin #sr-app {padding:18px}
 #sockrocket-plugin #sr-titlebar {flex-wrap:wrap;gap:10px}
 #sockrocket-plugin .logo {font-size:24px}
 #sockrocket-plugin .node-badge {max-width:100%;flex-basis:100%}
 #sockrocket-plugin .nav-btn {min-height:42px;padding:8px 10px}
 #sockrocket-plugin .nav-btn svg {display:none}
 #sockrocket-plugin .network-summary {grid-template-columns:1fr}
 #sockrocket-plugin .hero {flex-wrap:wrap}
 #sockrocket-plugin .hero-info {min-width:180px}
 #sockrocket-plugin .hero-title {font-size:18px}
 #sockrocket-plugin .hero>.btn {margin-left:60px}
 #sockrocket-plugin .card {padding:18px}
 #sockrocket-plugin .adaptive-form,#sockrocket-plugin .form-grid {grid-template-columns:1fr!important}
 #sockrocket-plugin .stat {padding:15px}
 #sockrocket-plugin .stat .value {font-size:22px}
 #sockrocket-plugin .rg-actions {margin-left:0}
 #sockrocket-plugin .row .btn {min-height:38px}
 #sockrocket-plugin .sb-right {display:none}
 #sockrocket-plugin .diag-row {align-items:flex-start}
 #sockrocket-plugin .diag-row>span:last-child {text-align:right}
 #sockrocket-plugin dialog {padding:22px}
}
</style>
</head>
<body class="bg" onload="sockrocketInitHost()">
<div id="TopBanner"></div><div id="Loading" class="popup_bg"></div>
<form name="form" hidden onsubmit="return false">
<input type="hidden" name="current_page" value="Module_sockrocket.asp">
<input type="hidden" name="next_page" value="Module_sockrocket.asp">
<input type="hidden" name="preferred_lang" id="preferred_lang" value="">
<input type="hidden" name="firmver" value="">
</form>
<table id="sr-firmware-layout" role="presentation"><tr>
<td class="sr-firmware-nav"><div id="mainMenu"></div><div id="subMenu"></div></td>
<td class="sr-firmware-content"><div id="tabMenu" class="submenuBlock"></div>
<div id="sr-host-notice" hidden>固件导航暂不可用，请从路由器软件中心打开 Sockrocket。</div>
<section id="sockrocket-plugin" aria-label="Sockrocket settings">
<div id="sr-app">
  <!-- ── Title bar ─────────────────────────────────────────────────── -->
  <p class="breadcrumb">软件中心 / Sockrocket <a class="router-home" href="/">路由器首页</a></p>
  <div id="sr-titlebar">
    <div class="logo">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 13.5 C7 9 8.6 5.2 12 3 C15.4 5.2 17 9 17 13.5"/><path d="M5.5 15.5 C8.5 14.4 15.5 14.4 18.5 15.5"/><path d="M10 17.5 L10 19.5"/><path d="M14 17.5 L14 21.5"/></svg>
      Sockrocket
    </div><span class="service-pill" id="sr-service-pill">等待状态</span>
    <nav class="project-links" aria-label="项目链接">
      <a href="https://t.me/sockrocket" target="_blank" rel="noopener noreferrer">Telegram</a>
      <a href="https://sockrockets.github.io/sockrocket/" target="_blank" rel="noopener noreferrer">官网</a>
    </nav>
    <div class="spacer"></div>
    <a class="btn" id="sr-exit-btn" href="/Module_Softcenter.asp#/" target="_top" title="返回软件中心">退出</a>
    <button type="button" class="node-badge" id="sr-node-badge" onclick="srShowPage('nodes')" title="查看当前节点">
      <span class="nb-name" id="sr-nb-name">—</span>
      <span class="nb-lat" id="sr-nb-lat"></span>
    </button>
  </div>

  <!-- ── Main row ──────────────────────────────────────────────────── -->
  <div id="sr-main">
    <nav id="sr-sidebar" role="tablist" aria-label="Sockrocket 设置">
      <button type="button" class="nav-btn active" id="sr-nav-home" role="tab" aria-controls="sr-page-home" aria-selected="true" tabindex="0" title="Overview" onclick="srShowPage('home')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.75 6A2.25 2.25 0 016 3.75h2.25A2.25 2.25 0 0110.5 6v2.25a2.25 2.25 0 01-2.25 2.25H6a2.25 2.25 0 01-2.25-2.25V6zM14 6a2.25 2.25 0 012.25-2.25H18A2.25 2.25 0 0120.25 6v2.25A2.25 2.25 0 0118 10.5h-2.25A2.25 2.25 0 0114 8.25V6zM3.75 15.75A2.25 2.25 0 016 13.5h2.25a2.25 2.25 0 012.25 2.25V18a2.25 2.25 0 01-2.25 2.25H6A2.25 2.25 0 013.75 18v-2.25zM13.5 15.75A2.25 2.25 0 0115.75 13.5H18a2.25 2.25 0 012.25 2.25V18a2.25 2.25 0 01-2.25 2.25h-2.25A2.25 2.25 0 0113.5 18v-2.25z"/></svg>
      <span>概览</span></button>
      <button type="button" class="nav-btn" id="sr-nav-nodes" role="tab" aria-controls="sr-page-nodes" aria-selected="false" tabindex="-1" title="Nodes" onclick="srShowPage('nodes')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 013 12c0-1.605.42-3.113 1.157-4.418"/></svg>
      <span>节点</span></button>
      <button type="button" class="nav-btn" id="sr-nav-subs" role="tab" aria-controls="sr-page-subs" aria-selected="false" tabindex="-1" title="Subscriptions" onclick="srShowPage('subs')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 11a9 9 0 019 9M4 4a16 16 0 0116 16M5 19a1 1 0 100-2 1 1 0 000 2z"/></svg>
      <span>订阅</span></button>
      <button type="button" class="nav-btn" id="sr-nav-dns" role="tab" aria-controls="sr-page-dns" aria-selected="false" tabindex="-1" title="Routing rules" onclick="srShowPage('dns')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a15 15 0 010 18M12 3a15 15 0 000 18"/></svg>
      <span>分流规则</span></button>
      <button type="button" class="nav-btn" id="sr-nav-network" role="tab" aria-controls="sr-page-network" aria-selected="false" tabindex="-1" title="Network and DNS" onclick="srShowPage('network')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="3" y="3" width="18" height="6" rx="2"/><rect x="3" y="15" width="18" height="6" rx="2"/><path d="M12 9v6M6 6h1M6 18h1"/></svg><span>网络与 DNS</span></button>
      <button type="button" class="nav-btn" id="sr-nav-logs" role="tab" aria-controls="sr-page-logs" aria-selected="false" tabindex="-1" title="Logs" onclick="srShowPage('logs')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6.75 7.5l3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0021 18V6a2.25 2.25 0 00-2.25-2.25H5.25A2.25 2.25 0 003 6v12a2.25 2.25 0 002.25 2.25z"/></svg>
      <span>诊断</span></button>      <button type="button" class="nav-btn" id="sr-nav-config" role="tab" aria-controls="sr-page-config" aria-selected="false" tabindex="-1" title="Config" onclick="srShowPage('config')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 20l-5.447-2.724A1 1 0 013 16.382V5.618a1 1 0 011.447-.894L9 7m0 13l6-3m-6 3V7m6 10l5.447 2.724A1 1 0 0021 18.382V7.618a1 1 0 00-.553-.894L15 7m0 13V7"/></svg>
      <span>高级设置</span></button>

    </nav>

    <main id="sr-content">
      <!-- ── Page: Home ────────────────────────────────────────────── -->
      <div class="page active" id="sr-page-home" role="tabpanel" aria-labelledby="sr-nav-home">
        <div class="page-title">概览</div><p class="page-intro">管理路由器连接，设置将应用于使用此路由器的设备。</p>
        <div class="card">
          <div class="hero">
            <div class="hero-icon off" id="sr-hero-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.75 13.5l10.5-11.25L12 10.5h8.25L9.75 21.75 12 13.5H3.75z"/></svg>
            </div>
            <div class="hero-info">
              <div class="hero-title"><span id="sr-hero-state">已停止</span><span class="live" id="sr-hero-live" style="display:none">运行中</span></div>
              <div class="hero-sub" id="sr-hero-node">—</div>
              <div class="hero-sub" id="sr-hero-health" style="display:none"></div>
              <div class="hero-metrics">
                <span>运行时间 <b id="sr-hero-uptime">—</b></span>
                <span>节点 <b id="sr-hero-nodes">0</b></span>
                <span>订阅 <b id="sr-hero-subs">0</b></span>
              </div>
            </div>
            <button class="btn btn-success" id="sr-toggle-btn" onclick="toggleService()">启动服务</button>
          </div>
        </div>

        <div class="network-summary" role="status"><span>服务进程 <b id="sr-health-process">等待状态</b></span><span>路由器探测 <b id="sr-health-outbound">尚未检查</b></span><span>局域网设备 <b>需要设备侧验证</b></span><span>配置 <b id="sr-config-apply">待确认</b></span><span id="sr-status-freshness">正在读取状态…</span></div>
        <div class="stat-grid">
          <div class="stat"><div class="label">活跃连接</div><div class="value" id="sr-st-connections">—</div><div class="sub" id="sr-st-connections-sub">等待运行数据</div></div>
          <div class="stat">
            <div class="label">内存使用</div>
            <div class="value" id="sr-st-mem">—</div>
            <div class="sub" id="sr-st-mem-sub">—</div>
          </div>
          <div class="stat"><div class="label">最近探测</div><div class="value" id="sr-st-probe">—</div><div class="sub" id="sr-st-probe-sub">尚未检查</div></div>
          <div class="stat"><div class="label">运行时间</div><div class="value" id="sr-st-service-uptime" style="font-size:19px">—</div><div class="sub">代理服务持续运行时间</div></div>
        </div>
        <section class="card" id="sr-traffic-card"><h3>本次启动流量</h3><p class="hint" style="margin-bottom:12px">只统计本次进程启动以来、各节点实际转发的用量。重启后重新累计。</p><div id="sr-traffic-list"><div class="hint">暂无流量记录</div></div></section>
<div class="card"><h3>网络设置</h3><p class="hint" style="margin-bottom:12px">启动服务时默认开启 DNS 接管与 TUN；停止服务会清理两者。运行中可分别关闭。</p><div class="toolbar">
          <button type="button" class="chip toggle" id="sr-chip-dns" onclick="toggleDns()" title="控制局域网 DNS 接管。关闭后使用运营商 DNS；开启且未启用透明代理时返回短 TTL 真实地址。">
            <span class="dot" id="sr-dns-dot"></span>DNS 接管 <b id="sr-dns-state">—</b>
          </button>
          <button type="button" class="chip toggle" id="sr-chip-fw" onclick="toggleProxy()" title="控制 TUN 透明代理。单独关闭它会保留 DNS 接管；恢复直连请使用诊断页的停止操作。">
            <span class="dot" id="sr-fw-dot"></span>透明代理 <b id="sr-fw-state">—</b>
          </button>
          <button type="button" class="chip toggle" id="sr-chip-cn" onclick="toggleCnDirect()" disabled>
            <span class="dot" id="sr-cn-dot"></span>国内 IP 绕过 TUN <b id="sr-cn-state">—</b>
          </button>
          <div class="mode-seg" id="sr-mode-seg" title="选择规则、全局或直连模式">
            <button type="button" id="sr-mode-rule" onclick="setProxyMode('rule')">规则</button>
            <button type="button" id="sr-mode-global" onclick="setProxyMode('global')">全局</button>
            <button type="button" id="sr-mode-direct" onclick="setProxyMode('direct')">直连</button>
          </div>
          <div class="chip"><span class="dot acc"></span>出口 IP <b id="sr-wan-ip">—</b></div>
        </div>

</div>
        <div class="toolbar" style="margin-top:12px">
          <button class="btn btn-accent btn-sm" onclick="runSpeedtest()">节点测速</button>
          <button class="btn btn-sm" onclick="runIpCheck()">检查出口 IP</button>
          <button class="btn btn-sm" onclick="restartService()">重启服务</button>
          <div class="spacer"></div>
          <button class="btn btn-sm" onclick="srShowPage('logs');runDiagnostics()">运行诊断</button>
        </div>


      </div>

      <!-- ── Page: Nodes ───────────────────────────────────────────── -->
      <div class="page" id="sr-page-nodes" role="tabpanel" aria-labelledby="sr-nav-nodes">
        <div class="page-title">节点</div><p class="page-intro">查看详情不会切换节点；只有“使用”才会改变连接。测速不会切换当前节点。</p>
        <section class="card"><h3>节点分组</h3><p class="hint">为不同规则选择独立出口。分组内选择节点不会改变全局节点；节点更名后仍保留成员关系。失效成员不会自动改走其他线路。</p><div class="toolbar"><button class="btn btn-sm" onclick="editProxyGroup(-1)">新建分组</button><button class="btn btn-sm" onclick="loadProxyGroups()">刷新分组</button></div><div id="sr-proxy-group-list" style="margin-top:12px"></div>
        <div id="sr-proxy-group-editor" hidden><h4>编辑节点分组</h4><div class="form-grid proxy-group-grid"><div class="fg"><label for="sr-pg-name">名称（必填）</label><input id="sr-pg-name" type="text" maxlength="96" placeholder="例如：视频线路"></div><div class="fg"><label for="sr-pg-type">选择方式</label><select id="sr-pg-type" onchange="updateProxyGroupMode()"><option value="select">手动选择</option><option value="fallback">TCP 建连失败时按成员顺序尝试</option><option value="url-test">按连接延迟自动选择</option></select></div><div class="fg"><label id="sr-pg-members-label">组内成员</label><div id="sr-pg-members" class="pg-member-list" role="group" aria-labelledby="sr-pg-members-label"></div><span class="hint" id="sr-pg-member-hint">可勾选多个成员，再选择当前使用的节点。保存后生效。</span><div class="toolbar" id="sr-pg-member-tools"><button type="button" class="btn btn-sm" onclick="setProxyGroupMembers(true)">全选</button><button type="button" class="btn btn-sm" onclick="setProxyGroupMembers(false)">清空</button></div></div><div class="fg" id="sr-pg-current-field"><label for="sr-pg-current">当前使用节点</label><select id="sr-pg-current" onchange="pickProxyGroupMember(this.value)"></select><span class="hint">从已勾选成员中选择一个出口，保存后生效；不会自动切换。</span></div></div><div class="toolbar"><button class="btn btn-accent" id="sr-pg-save" onclick="saveProxyGroup()">保存分组</button><button class="btn" onclick="srElement('proxy-group-editor').hidden=true">取消</button></div></div></section>
        <div class="toolbar">
          <button class="btn btn-accent btn-sm" id="sr-test-all-btn" onclick="testAllNodes()">快速测速</button>
          <button class="btn btn-sm" onclick="testAllNodes('http')">真实延迟</button>
          <button class="btn btn-sm" onclick="srLoadNodes()">刷新</button><button class="btn btn-sm" onclick="toggleAddNodeForm()">添加节点</button>
          <div class="spacer"></div>
          <input type="search" id="sr-node-search" aria-label="搜索节点" placeholder="搜索名称或协议" oninput="renderNodes()"><span class="hint" id="sr-nodes-count"></span>
        </div>
        <div class="card collapsible" id="sr-add-node-card">
          <h3>添加节点</h3>
          <div class="form-grid">
            <div class="fg"><label for="sr-an-name">名称</label><input type="text" id="sr-an-name" placeholder="例如：常用线路"></div>
            <div class="fg"><label>协议</label>
              <select id="sr-an-proto" onchange="updateAddNodeFields()">
                <option value="shadowsocks">shadowsocks</option>
                <option value="vmess">vmess</option>
                <option value="trojan">trojan</option>
                <option value="vless">vless</option>
                <option value="hysteria2">hysteria2</option>
                <option value="tuic">tuic</option>
                <option value="anytls">AnyTLS</option>
                <option value="wireguard">WireGuard</option>
              </select>
            </div>
            <div class="fg"><label for="sr-an-server">服务器</label><input type="text" id="sr-an-server" placeholder="example.com"></div>
            <div class="fg"><label for="sr-an-port">端口</label><input type="number" id="sr-an-port" min="1" max="65535" step="1" placeholder="443"></div>
            <div class="fg"><label for="sr-an-pass">密码</label><input type="password" autocomplete="new-password" id="sr-an-pass" placeholder="(ss/trojan/hy2/tuic)"></div>
            <div class="fg"><label for="sr-an-uuid">UUID</label><input type="text" autocomplete="off" id="sr-an-uuid" placeholder="(vmess/vless/tuic)"></div>
            <div class="fg"><label for="sr-an-cipher">加密方式</label><input type="text" id="sr-an-cipher" placeholder="(ss/vmess, default aes-256-gcm/auto)"></div>
          </div>
          <div class="form-grid" id="sr-an-tls-fields" hidden>
            <div class="fg"><label for="sr-an-sni">TLS 服务器名称（可选）</label><input type="text" id="sr-an-sni" placeholder="默认使用服务器地址"></div>
            <div class="fg"><label><input type="checkbox" id="sr-an-insecure"> 跳过证书验证（仅自签名测试）</label></div>
          </div>
          <div id="sr-an-wg-fields" hidden>
            <p class="hint">原生用户态 WireGuard；每个节点对应一个 Peer，不改写系统路由。密钥仅保存在本机私有配置。</p>
            <div class="form-grid">
              <div class="fg"><label for="sr-an-wg-private">私钥</label><input type="password" autocomplete="new-password" id="sr-an-wg-private"></div>
              <div class="fg"><label for="sr-an-wg-public">Peer 公钥</label><input type="password" autocomplete="new-password" id="sr-an-wg-public"></div>
              <div class="fg"><label for="sr-an-wg-psk">预共享密钥（可选）</label><input type="password" autocomplete="new-password" id="sr-an-wg-psk"></div>
              <div class="fg"><label for="sr-an-wg-address">隧道地址（CIDR，逗号分隔）</label><input type="text" id="sr-an-wg-address" placeholder="例如 10.0.0.2/32"></div>
              <div class="fg"><label for="sr-an-wg-allowed">Peer 允许的网段</label><input type="text" id="sr-an-wg-allowed" value="0.0.0.0/0, ::/0"></div>
              <div class="fg"><label for="sr-an-wg-dns">隧道内 DNS（IP，逗号分隔）</label><input type="text" id="sr-an-wg-dns" value="1.1.1.1, 2606:4700:4700::1111"></div>
              <div class="fg"><label for="sr-an-wg-mtu">MTU</label><input type="number" id="sr-an-wg-mtu" min="576" max="1420" value="1280"></div>
              <div class="fg"><label for="sr-an-wg-keepalive">Keepalive 秒数（0 关闭）</label><input type="number" id="sr-an-wg-keepalive" min="0" max="65535" value="25"></div>
            </div>
          </div>
          <button class="btn btn-success btn-sm" onclick="addNode()">添加</button>
        </div>
        <div class="card">
          <div class="rows" id="sr-node-list"><div class="hint">正在加载…</div></div>
        </div>
      </div>

      <!-- ── Page: Subscriptions ───────────────────────────────────── -->
      <div class="page" id="sr-page-subs" role="tabpanel" aria-labelledby="sr-nav-subs">
        <div class="page-title">订阅</div><p class="page-intro">订阅更新失败会保留当前节点。地址默认隐藏，避免截图泄露。</p>
        <div class="card">
          <h3>添加订阅</h3>
          <div class="form-grid adaptive-form" style="grid-template-columns: minmax(120px,1fr) minmax(160px,2fr) 110px auto">
            <div class="fg"><label for="sr-sub-name">名称（选填）</label><input type="text" id="sr-sub-name" placeholder="留空自动命名"></div>
            <div class="fg"><label for="sr-sub-url">订阅地址（必填）</label><input type="url" required aria-required="true" autocomplete="off" spellcheck="false" id="sr-sub-url" placeholder="https://..."></div>
            <div class="fg"><label for="sr-sub-format">格式（选填）</label>
              <select id="sr-sub-format">
                <option value="auto" selected>自动识别</option>
                <option value="clash">clash</option>
                <option value="v2ray">v2ray (URI / base64)</option>
                <option value="singbox">sing-box</option>
                <option value="base64">base64</option>
              </select>
            </div>
            <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" id="sr-sub-add" onclick="addSub()">添加</button></div>
          </div>
          <p class="hint">只需填写订阅地址。名称留空自动命名，格式默认自动识别；批量更新会跳过错误地址并保留对应的已有节点。HTTP 地址会明文传输访问令牌和节点内容；供应商支持时请优先使用 HTTPS。</p>
        </div>
        <div class="card">
          <div class="toolbar">
            <h3 style="margin:0">我的订阅</h3><button class="btn btn-sm" onclick="srLoadSubs()">刷新</button>
            <div class="spacer"></div>
            <button class="btn btn-accent btn-sm" id="sr-sub-update" onclick="updateSubs()">全部更新</button>
          </div>
          <div class="rows" id="sr-sub-list"><div class="hint">正在加载…</div></div>
          <div class="hint">更新失败时保留现有节点。订阅地址默认隐藏，请勿分享包含地址的截图。</div>
        </div>
        <section class="card" id="sr-sub-traffic-card"><h3>近三个月节点用量</h3><p class="hint" style="margin-bottom:12px">每个节点按自然月单独累计，订阅里的节点也列在这里。直连和未归属订阅的节点同样显示。</p><div id="sr-sub-traffic-months"><div class="hint">暂无用量记录</div></div></section>
        <section class="card" id="sr-sub-link-traffic-card"><h3>近三个月订阅用量</h3><p class="hint" style="margin-bottom:12px">把上面属于同一订阅的节点加总。直连和未归属订阅的节点不算在内。只显示订阅名称，不显示订阅地址。</p><div id="sr-sub-link-traffic"><div class="hint">暂无用量记录</div></div></section>
        <details class="card" id="sr-sub-notes" hidden>
          <summary>兼容说明</summary>
          <p class="hint" id="sr-sub-notes-text"></p>
        </details>
        <div class="card" id="sr-sub-diagnostic" hidden>
          <h3>订阅失败诊断</h3><p class="hint">以下信息不包含订阅地址、节点凭证或个人路径，可复制用于反馈。</p>
          <pre class="rule-preview" id="sr-sub-diagnostic-text"></pre>
          <button type="button" class="btn btn-sm" onclick="copySubscriptionDiagnostic()">复制脱敏诊断</button>
        </div>
      </div>

      <!-- ── Page: Routing rules ─────────────────────────────────────── -->
      <div class="page" id="sr-page-dns" role="tabpanel" aria-labelledby="sr-nav-dns">
        <div class="page-title">分流规则</div><p class="page-intro">在规则模式下，已指定设备策略的来源使用自己的规则；其余来源使用全局设备的规则。随后检查国内直连及默认出口。</p>
        <section class="card" aria-labelledby="sr-rule-flow-title">
          <div class="rule-flow-head"><h3 id="sr-rule-flow-title">流量去向</h3><span class="flow-state" id="sr-rule-flow-state">等待状态</span><button type="button" class="btn btn-sm" id="sr-rule-flow-toggle" aria-expanded="true" aria-controls="sr-rule-flow" onclick="toggleRuleFlow()">收起概览</button></div>
          <p class="hint">设备和网段只连接各自配置的规则。共用规则从「全局设备」连出。选择设备并输入目标，可查看实际命中的路径。</p>
          <p class="hint" id="sr-rule-flow-summary" role="status"></p>
          <div class="flow-query">
            <div class="fg"><label for="sr-rule-flow-destination">网站 / 目的 IP</label><input id="sr-rule-flow-destination" type="text" placeholder="例如 example.com，也可粘贴网址" autocomplete="off" spellcheck="false" oninput="invalidateRuleTrace()" onkeydown="if(event.key==='Enter')traceRuleFlow()"></div>
            <div class="fg"><label for="sr-rule-flow-device">模拟设备</label><select id="sr-rule-flow-device" onchange="changeFlowDevice();syncSimulatedSource();if(this.value==='manual')srElement('rule-flow-client').focus()"><option value="">全局设备 · 配置总览</option></select><div id="sr-rule-flow-manual" hidden><input id="sr-rule-flow-client" onchange="syncSimulatedSource()" type="text" aria-label="手动输入设备 IP" placeholder="单个 IPv4 或 IPv6 地址" autocomplete="off" oninput="FLOW_EDIT_NETWORK='';FLOW_EDIT_GROUP=null;invalidateRuleTrace()" onkeydown="if(event.key==='Enter')traceRuleFlow()"></div></div>
            <div class="fg"><label for="sr-rule-flow-port">端口</label><input id="sr-rule-flow-port" type="number" min="1" max="65535" placeholder="自动" oninput="invalidateRuleTrace()" onkeydown="if(event.key==='Enter')traceRuleFlow()"></div>
          </div>
          <div class="toolbar"><button type="button" class="btn btn-accent btn-sm" id="sr-rule-flow-check" onclick="traceRuleFlow()">检查分流</button><button type="button" class="btn btn-sm" onclick="previewDeviceFlow()">只看配置流程</button><button type="button" class="btn btn-sm" onclick="resetDeviceFlow()">清除检查</button></div>
          <details><summary>DNS 与目的 IP</summary><div class="fg" style="margin-top:10px"><label for="sr-rule-flow-ip">真实目的 IP（选填，用于复现设备实际解析结果）</label><input id="sr-rule-flow-ip" type="text" autocomplete="off" placeholder="留空时按需解析" oninput="invalidateRuleTrace()"></div><label class="hint"><input id="sr-rule-flow-resolve" type="checkbox" checked onchange="invalidateRuleTrace()"> 遇到 IP / GeoIP 规则时查询 DNS</label></details>
          <p class="hint" id="sr-rule-flow-query-state" role="status" aria-live="polite">连线表示可能的出口，每次匹配只选一个。输入目的地址可检查命中路径；点击规则组可查看规则。</p>
          <div id="sr-rule-flow" aria-label="分流配置路径"><p class="hint">正在加载规则…</p></div>
        </section>
        <section class="card" id="sr-rule-workspace" aria-labelledby="sr-rule-workspace-title">
          <div class="toolbar"><h3 id="sr-rule-workspace-title">规则组维护</h3><div class="spacer"></div></div>
          <p class="hint" id="sr-rule-operation-state" role="status" aria-live="polite"></p>
          <div id="sr-rule-create-menu" class="rule-inline-editor" hidden>
          <div id="sr-rule-group-creator" class="rule-inline-editor" hidden><div class="fg"><label for="sr-rule-new-group-name">规则组名称</label><input id="sr-rule-new-group-name" type="text" placeholder="例如：常用服务" onkeydown="if(event.key==='Enter')createRuleGroup()"></div><p class="hint">添加第一条规则后保存分组，同一组可以包含多种类型的规则。</p><div class="toolbar"><button class="btn btn-accent btn-sm" id="sr-rule-group-new" onclick="createRuleGroup()">创建规则组</button><button class="btn btn-sm" onclick="closeRuleCreateMenu()">取消</button></div></div>
          </div>
          <div class="toolbar rule-workspace-actions">
            <div class="fg"><label for="sr-rule-group-select">选择分组</label><select id="sr-rule-group-select" onchange="keepRulePickerPosition(this, () => { changeRuleGroup(this.value); })" disabled><option value="">正在加载…</option></select></div>
            <button class="btn btn-sm" aria-controls="sr-rule-create-menu" aria-expanded="false" onclick="openRuleGroupCreator('blank')" id="sr-rule-create-toggle">新建规则组</button>
            <details><summary>分组设置</summary>
              <div class="toolbar"><button class="btn btn-sm" id="sr-rule-bulk-edit" onclick="changeRuleWorkspaceMode('bulk')">批量编辑</button><button class="btn btn-sm" onclick="exportRules()">导出规则</button><button class="btn btn-sm" onclick="srLoadRules()">刷新</button></div>
              <div class="toolbar"><button class="btn btn-sm" id="sr-rule-group-enable" onclick="setRuleGroupEnabled(encodeURIComponent(RULE_SELECTED_GROUP),true)">启用该组</button><button class="btn btn-sm" id="sr-rule-group-disable" onclick="setRuleGroupEnabled(encodeURIComponent(RULE_SELECTED_GROUP),false)">停用该组</button><button class="btn btn-danger btn-sm" id="sr-rule-group-delete" onclick="deleteRuleGroup(encodeURIComponent(RULE_SELECTED_GROUP))">删除该组</button></div>
            </details>
            <button class="btn btn-accent btn-sm" id="sr-rule-add" onclick="startRuleAddition()">添加规则</button>
          </div>
          <p class="hint" id="sr-rule-group-summary" aria-live="polite"></p>
          <datalist id="sr-rule-group-list"></datalist><input type="hidden" id="sr-rule-group"><input type="hidden" id="sr-rule-entry-mode" value="single">
          <div id="sr-rule-add-panel" class="rule-inline-editor" hidden>
            <div class="toolbar"><h3 id="sr-rule-editor-state">添加规则</h3><button class="btn btn-sm" id="sr-rule-edit-cancel" onclick="cancelRuleEditor()">取消</button></div>
            <div class="toolbar" id="sr-rule-add-methods" aria-label="添加方式">
              <button class="btn btn-sm" id="sr-rule-method-manual" onclick="startRuleAddition()">手动填写</button>
              <button class="btn btn-sm" id="sr-rule-import" onclick="startRuleImport()">从链接导入</button>
              <button class="btn btn-sm" id="sr-rule-method-preset" onclick="startRulePresetAddition()">使用预设</button>
            </div>
          <div id="sr-rule-preset-panel" hidden>
            <p class="hint">导入后可在对应分组查看、编辑和启用规则。</p>
            <div class="toolbar" style="margin-top:12px"><div class="fg" style="flex:1;min-width:180px"><label for="sr-rule-preset">选择常用预设</label><select id="sr-rule-preset" onchange="renderRulePreset()" disabled><option value="">正在加载…</option></select></div><span class="badge" id="sr-rule-preset-target"></span><div class="fg"><label for="sr-preset-route-target">添加时的出口</label><select id="sr-preset-route-target"><option value="">使用预设默认</option></select></div></div>
            <p class="hint" id="sr-rule-preset-description"></p>
            <p class="hint" id="sr-rule-preset-state" role="status" aria-live="polite"></p>
            <div class="toolbar" style="margin-top:10px"><button class="btn btn-sm" id="sr-rule-preset-add" disabled onclick="addRulePreset()">添加为停用规则</button><button class="btn btn-sm" id="sr-rule-preset-view" hidden onclick="viewRulePreset()">查看已添加规则</button><button class="btn btn-accent btn-sm" id="sr-rule-preset-enable" hidden onclick="toggleRulePreset(true)">开启该组</button><button class="btn btn-sm" id="sr-rule-preset-disable" hidden onclick="toggleRulePreset(false)">停用该组</button></div>
            <details style="margin-top:10px"><summary>查看预设域名及匹配方式</summary><p class="hint">DOMAIN 匹配完整域名，DOMAIN-SUFFIX 同时匹配子域名。初始优先级 −10。通常低于手动规则的默认优先级 0；设备规则仍优先。添加后可单条或按组编辑，开启以当前规则内容为准。</p><pre class="rule-preview" id="sr-rule-preset-domains"></pre></details>
          </div>
            <div id="sr-rule-input-fields">
            <div id="sr-rule-import-panel" hidden>
              <div class="fg"><label for="sr-rule-import-url">规则列表链接</label><input type="url" id="sr-rule-import-url" maxlength="4096" autocomplete="off" spellcheck="false" placeholder="https://example.com/rules.list"></div>
              <div class="toolbar" style="margin-top:8px"><button class="btn btn-sm" id="sr-rule-import-fetch" onclick="fetchRuleList()">读取并预览</button><span class="hint" id="sr-rule-import-state" role="status" aria-live="polite"></span></div>
              <p class="hint">读取后可修改并预览，再追加到当前组。未指定出口的规则使用下方所选出口；注释和空行忽略，重复规则跳过。仅导入本次内容，不自动更新。</p>
            </div>
          <div class="form-grid adaptive-form" id="sr-rule-default-fields">
            <div class="fg"><label for="sr-rule-type">类型</label>
              <select id="sr-rule-type" onchange="ruleTypeChanged()">
                <option value="domain-suffix">域名后缀</option>
                <option value="domain">完整域名</option>
                <option value="domain-keyword">域名关键词</option>
                <option value="ip-cidr">IP 网段</option>
                <option value="geoip">地区</option>
                <option value="dst-port">端口</option>
                <option value="final">最终规则</option>
              </select>
            </div>
            <div class="fg"><label id="sr-rule-target-label" for="sr-rule-target">处理方式</label>
              <select id="sr-rule-target">
                <option value="proxy">代理</option>
                <option value="direct">直连</option>
                <option value="reject">拒绝</option>
              </select>
            </div>
            <div class="fg"><label for="sr-rule-source">设备来源 IP / 网段</label><input type="text" id="sr-rule-source" placeholder="留空作为组内规则" autocomplete="off"><span class="hint">填写固定 DHCP IP；设备组可填 CIDR。Merlin LAN 透明接管仅支持 IPv4。</span></div>
            <div class="fg"><label for="sr-rule-priority">优先级</label><input type="number" id="sr-rule-priority" value="0" step="1" min="-2147483648" max="2147483647" placeholder="留空保留现有值"></div>
            <div class="fg"><label>重复规则</label>
              <select id="sr-rule-dup">
                <option value="skip">跳过</option>
                <option value="replace">替换</option>
              </select>
            </div>
          </div>
          <div class="hint" id="sr-rule-input-hint" style="margin-bottom:6px">每行一条，支持域名及其子域名。</div>
          <div id="sr-rule-single-wrap" style="display:none">
            <div class="form-grid adaptive-form" style="grid-template-columns: 1fr auto">
              <div class="fg"><label for="sr-rule-pattern">匹配内容</label><input type="text" id="sr-rule-pattern" placeholder="example.invalid"></div>
              <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" onclick="previewRuleChanges()">预览规则</button></div>
            </div>
          </div>
          <div id="sr-rule-bulk-wrap">
            <textarea id="sr-rule-batch" aria-label="待导入或批量编辑的规则" rows="5" spellcheck="false" placeholder="DOMAIN-SUFFIX,example.invalid,PROXY&#10;GEOIP,CN,DIRECT&#10;MATCH,PROXY"></textarea>
            <div class="toolbar" style="margin-top:8px">
              <div class="spacer"></div>
              <span class="hint">域名、网段、地区可每行一个值；混合规则使用 TYPE,内容,ACTION。</span>
              <button class="btn btn-success btn-sm" onclick="previewRuleChanges()">预览规则</button>
            </div>
          </div>
          </div>
          </div>
<section hidden class="rule-inline-editor" id="sr-rule-edit-panel" aria-labelledby="sr-rule-edit-title"><h3 id="sr-rule-edit-title">编辑规则</h3><p class="hint">可直接修改优先级，数字越大越先匹配；改为最终规则时会在国内直连之后匹配。运行中的服务会重启以应用规则。</p><div class="form-grid"><div class="fg"><label for="sr-edit-rule-group">分组</label><input type="text" id="sr-edit-rule-group" list="sr-rule-group-list"></div><div class="fg"><label for="sr-edit-rule-type">类型</label><select id="sr-edit-rule-type" onchange="editRuleTypeChanged()"><option value="domain">完整域名</option><option value="domain-suffix">域名后缀</option><option value="domain-keyword">域名关键词</option><option value="ip-cidr">IP 网段</option><option value="geoip">地区</option><option value="dst-port">端口</option><option value="final">最终规则</option></select></div><div class="fg"><label for="sr-edit-rule-pattern">匹配内容</label><input type="text" id="sr-edit-rule-pattern" spellcheck="false"></div><div class="fg"><label for="sr-edit-rule-source">设备来源 IP / 网段</label><input id="sr-edit-rule-source" type="text" placeholder="留空作为组内规则" autocomplete="off"></div><div class="fg"><label for="sr-edit-rule-priority">优先级（数字越大越先匹配）</label><input type="number" id="sr-edit-rule-priority" step="1" min="-2147483648" max="2147483647"></div><div class="fg"><label for="sr-edit-rule-target">处理方式</label><select id="sr-edit-rule-target"><option value="proxy">代理</option><option value="direct">直连</option><option value="reject">拒绝</option></select></div></div><label><input type="checkbox" id="sr-edit-rule-enabled"> 启用此规则</label><div class="dialog-actions"><button class="btn" onclick="cancelRuleRowEditor()">取消</button><button class="btn btn-accent" id="sr-rule-edit-save" onclick="saveRuleEdit()">保存修改</button></div></section>
          <div id="sr-rule-list-panel">
          <input type="text" id="sr-rule-search" autocomplete="off" spellcheck="false" aria-label="搜索规则" placeholder="查找组内规则（域名、IP 或名称）" oninput="renderRules()">
          <details id="sr-rule-filter-panel"><summary>更多筛选</summary><div class="rule-filters"><select id="sr-rule-filter-target" aria-label="筛选处理方式" onchange="renderRules()"><option value="">全部处理方式</option><option value="proxy">代理</option><option value="direct">直连</option><option value="reject">拒绝</option></select><select id="sr-rule-filter-scope" aria-label="筛选规则来源" onchange="renderRules()"><option value="">全部来源</option><option value="global">全局规则</option><option value="device">设备专属规则</option></select><select id="sr-rule-filter-state" aria-label="筛选状态" onchange="renderRules()"><option value="">全部状态</option><option value="enabled">已启用</option><option value="disabled">已停用</option></select></div><button class="btn btn-sm" onclick="changeRuleGroup(RULE_SELECTED_GROUP)">清除筛选</button></details><p class="hint" id="sr-rule-count" aria-live="polite"></p>
          <div class="rows" id="sr-rule-list"><div class="hint">正在加载…</div></div>
          <div class="toolbar" id="sr-rule-pager" hidden><span class="hint" id="sr-rule-page-label"></span><div class="spacer"></div><button class="btn btn-sm" id="sr-rule-page-prev" onclick="changeRulePage(-1)">上一页</button><button class="btn btn-sm" id="sr-rule-page-next" onclick="changeRulePage(1)">下一页</button></div>
          <details><summary>匹配顺序说明</summary><p class="hint">每个分组可以指定自己的代理节点。直连和拒绝的规则保持不变。按优先级从高到低匹配；相同优先级按配置顺序匹配。已指定策略的设备依次检查专属规则、关联组、内置国内直连，再走设备默认出口（默认为代理）。其余设备检查全局设备的规则、内置国内直连和全局设备的默认出口。上下移动仅限同一匹配阶段，会更新规则优先级，并针对完整列表操作，不受筛选或分页影响。</p></details>
          </div>
        </section>
        <section class="card" aria-labelledby="sr-rule-application-title">
          <div class="toolbar"><h3 id="sr-rule-application-title">设备分流策略</h3><div class="spacer"></div><button class="btn btn-sm" onclick="srLoadRules()">刷新设备</button></div>
          <fieldset id="sr-device-form" disabled style="border:0;padding:0;margin:0;min-width:0">
            <div class="form-grid">
              <div class="fg"><label for="sr-device-picker">选择设备</label><select id="sr-device-picker" aria-describedby="sr-device-scope-note" onchange="keepRulePickerPosition(this, () => { selectPolicyDevice();syncEditorToMap(); })"><option value="global">全局设备</option></select><span class="hint" id="sr-device-scope-note">适用于未单独配置策略的设备</span></div>
              <div class="fg" id="sr-device-manual" hidden><label for="sr-device-source">来源 IP / 网段</label><input id="sr-device-source" placeholder="例如 192.168.50.20" oninput="updateDevicePolicySelection()" onchange="syncEditorToMap()"></div>
            </div>
            <div id="sr-device-settings" hidden>
              <div class="toolbar"><h4 id="sr-device-binding-title">使用的规则组</h4><div class="spacer"></div><button type="button" class="btn btn-sm" onclick="switchDevicePanel('add')">关联规则组</button><button type="button" class="btn btn-sm" onclick="switchDevicePanel('fallback')">默认出口</button></div>
              <div id="sr-device-bindings" class="rows"></div>
              <details id="sr-device-inherited" hidden><summary id="sr-device-inherited-title"></summary><ul id="sr-device-inherited-list"></ul><button type="button" class="btn btn-sm" onclick="srElement('device-picker').value='global';selectPolicyDevice();syncEditorToMap()">查看全局设备策略</button></details>
                <div id="sr-device-bound-editor" class="rule-inline-editor" hidden>
                  <div class="toolbar"><strong id="sr-binding-name"></strong><div class="spacer"></div><button type="button" class="btn btn-sm" onclick="closeDeviceBinding()">取消</button></div>
                  <div class="form-grid">
                    <input type="hidden" id="sr-device-group">
                    <div class="fg"><label for="sr-device-group-priority">关联优先级</label><input id="sr-device-group-priority" type="number" min="0" max="65535" step="1" value="0"></div>
                    <div class="fg"><label for="sr-device-group-target">命中该组时</label><select id="sr-device-group-target"></select></div>
                  </div>
                  <p class="hint">多个组同时命中时，关联优先级高的先匹配。同级沿用规则优先级和原有顺序。</p>
                  <p id="sr-device-group-state" class="hint" aria-live="polite"></p>
                  <div class="toolbar"><button class="btn btn-accent btn-sm" id="sr-device-link" onclick="saveDeviceBinding(false)">保存绑定</button><button class="btn btn-sm" id="sr-device-unlink" onclick="saveDeviceBinding(true)">解除该组关联</button></div>
                </div>
                <div id="sr-device-add-groups" class="rule-inline-editor" hidden><div class="toolbar"><strong>关联规则组（可多选）</strong><div class="spacer"></div><button type="button" class="btn btn-sm" onclick="cancelDevicePanel()">取消</button></div>
                  <div class="pg-member-list" id="sr-device-available-groups" onchange="updateDeviceAddCount()"></div>
                  <div class="toolbar"><div class="fg"><label for="sr-device-add-target">命中规则时的出口</label><select id="sr-device-add-target"><option value="">请选择出口（必填）</option></select></div><div class="fg"><label for="sr-device-add-priority">新关联优先级（数字越大越先匹配）</label><input id="sr-device-add-priority" type="number" min="0" max="65535" step="1" value="0"></div><button type="button" id="sr-device-add-selected" class="btn btn-accent btn-sm" disabled onclick="addDeviceGroups()">关联所选组</button></div>
                </div>
              <div id="sr-device-fallback" class="rule-inline-editor" hidden><div class="toolbar"><strong id="sr-device-fallback-label">未命中规则时的处理方式</strong><div class="spacer"></div><button type="button" class="btn btn-sm" onclick="cancelDevicePanel()">取消</button></div><div class="toolbar"><div class="fg"><label for="sr-device-target">处理方式</label><select id="sr-device-target" aria-describedby="sr-device-effective-policy"></select><span class="hint" id="sr-device-effective-policy" hidden></span></div><button class="btn btn-sm" onclick="saveDevicePolicy()">保存</button></div></div>
              <div id="sr-device-specific">
                <div class="toolbar"><button type="button" class="btn btn-sm" id="sr-device-trace" onclick="inspectDeviceTraffic()">检查这台设备的分流</button><button type="button" class="btn btn-danger btn-sm" onclick="deletePolicyDevice()">删除设备配置</button></div>
                <button type="button" class="btn btn-sm" onclick="switchDevicePanel('alias')">修改设备名称</button><div id="sr-device-alias-panel" class="rule-inline-editor" hidden><div class="toolbar"><input id="sr-device-alias" aria-label="自定义设备名称" maxlength="48" placeholder="例如 客厅电视"><button class="btn btn-sm" onclick="saveDeviceAlias()">保存名称</button><button type="button" class="btn btn-sm" onclick="cancelDevicePanel()">取消</button></div></div>
              </div>

            </div>
          </fieldset>

        </section>
      </div>

      <div class="page" id="sr-page-network" role="tabpanel" aria-labelledby="sr-nav-network">
        <div class="page-title">网络与 DNS</div>
        <p class="hint" id="sr-network-state" role="status">正在读取配置…</p>
        <div class="card"><h3>地址与转发</h3>
          <div class="form-grid adaptive-form">
            <div class="fg"><label for="sr-ipv6-mode">IPv6 转发模式（必选）</label><select id="sr-ipv6-mode" onchange="networkPolicyEdited()"><option value="compatibility">推荐 · IPv4 兼容（不依赖运营商 IPv6）</option><option value="dual_stack">可选 · IPv6 双栈（固件已有 IPv6 时）</option></select></div>
            <div class="fg"><label for="sr-dns-strategy">地址选择（必选）</label><select id="sr-dns-strategy" onchange="networkPolicyEdited()"><option value="prefer_ipv4">优先 IPv4</option><option value="prefer_ipv6">优先 IPv6</option><option value="ipv4_only">仅 IPv4</option><option value="ipv6_only">仅 IPv6</option></select></div>
          </div><p class="hint">优先策略用于程序自身解析与拨号；LAN 设备的双栈连接顺序由设备决定。“仅 IPv4 / IPv6”会限制 DNS 应答。</p><p class="hint">不确定运营商是否开通 IPv6 时，请保持「IPv4 兼容」：代理走 IPv4，固件若已开启 IPv6 则拒绝外网 IPv6 转发，设备自动回退到 IPv4。只有固件已经拿到 IPv6 地址和 LAN 前缀时，再选「双栈」接管 LAN 的 IPv6；做不完时仍保持 IPv4 代理并自动重试，不会要求你改运营商设置。</p>
        </div>
        <div class="card"><h3>DNS 上游</h3><p class="hint">每行一个完整地址。直连支持 udp://、tcp://、tls://、https://；代理支持 tcp://、tls://、https://。加密 DNS 始终校验证书；主机名通过引导 DNS 解析。</p>
          <div class="fg"><label for="sr-dns-direct">直连 DNS（必填，1–8 个）</label><textarea id="sr-dns-direct" rows="3" spellcheck="false" placeholder="udp://223.5.5.5:53" oninput="networkPolicyEdited()"></textarea></div>
          <div class="fg"><label for="sr-dns-proxy">代理 DNS（必填，1–8 个）</label><textarea id="sr-dns-proxy" rows="3" spellcheck="false" placeholder="tls://dns.google:853" oninput="networkPolicyEdited()"></textarea></div>
          <div class="fg"><label for="sr-dns-bootstrap">引导与节点 DNS（必填，1–4 个 IP:端口）</label><textarea id="sr-dns-bootstrap" rows="2" spellcheck="false" placeholder="223.5.5.5:53&#10;[2001:db8::53]:53" oninput="networkPolicyEdited()"></textarea></div>
          <p class="hint">不要填写路由器自身或本机监听地址。代理 DNS 失败不会自动改用直连 DNS。</p>
          <details><summary>缓存与域名例外</summary>
            <div class="form-grid adaptive-form" style="margin-top:10px">
              <div class="fg"><label for="sr-dns-cache">解析缓存</label><select id="sr-dns-cache" onchange="networkPolicyEdited()"><option value="true">开启</option><option value="false">关闭</option></select></div>
              <div class="fg"><label for="sr-dns-capacity">缓存条数（16–16384）</label><input id="sr-dns-capacity" type="number" min="16" max="16384" oninput="networkPolicyEdited()"></div>
              <div class="fg"><label for="sr-dns-ttl">缓存 TTL（0 为上游 TTL，最多 86400 秒）</label><input id="sr-dns-ttl" type="number" min="0" max="86400" oninput="networkPolicyEdited()"></div>
              <div class="fg"><label for="sr-dns-timeout">查询超时（1–12 秒）</label><input id="sr-dns-timeout" type="number" min="1" max="12" oninput="networkPolicyEdited()"></div>
            </div>
            <div class="fg"><label for="sr-dns-domains">额外直连解析域名（选填，每行一个后缀）</label><textarea id="sr-dns-domains" rows="3" spellcheck="false" placeholder="example.test" oninput="networkPolicyEdited()"></textarea></div>
          </details>
          <div class="toolbar"><button type="button" class="btn btn-success" id="sr-network-save" onclick="saveNetworkPolicy()" disabled>保存并应用</button><button type="button" class="btn" id="sr-network-reload" onclick="loadNetworkPolicy()">重新加载</button></div>
          <p class="hint">运行中会就地重新应用防火墙策略，代理不重启；停止时只保存，不会启动代理。订阅、节点和其他配置会保留。</p>
        </div>
        <div class="card"><h3>DNS 测试</h3><p class="hint">测试当前表单，不保存更改。此处验证上游解析，实际 LAN 转发需要单独验证。</p>
          <div class="fg"><label for="sr-dns-test-domain">域名（必填）</label><input id="sr-dns-test-domain" oninput="dnsTestGeneration++" autocomplete="off" placeholder="example.com"></div>
          <div class="toolbar"><select id="sr-dns-test-group" onchange="dnsTestGeneration++" aria-label="测试分组"><option value="direct">直连 DNS</option><option value="proxy">代理 DNS</option><option value="bootstrap">引导 DNS</option></select><select id="sr-dns-test-type" onchange="dnsTestGeneration++" aria-label="查询类型"><option>A</option><option>AAAA</option></select><button type="button" class="btn" id="sr-dns-test" onclick="testDnsPolicy()">测试解析</button></div>
          <p class="hint" id="sr-dns-test-result" role="status">不会保存查询历史。</p>
        </div>
      </div>

      <!-- ── Page: Config ──────────────────────────────────────────── -->
      <div class="page" id="sr-page-config" role="tabpanel" aria-labelledby="sr-nav-config">
        <div class="page-title">高级配置</div>
        <div class="toolbar">
          <button class="btn btn-success btn-sm" id="sr-config-save" onclick="srSaveConfig()">校验并保存</button>
          <button class="btn btn-sm" onclick="srLoadConfig()">重新加载</button>
          <span class="hint">保存前自动校验，校验失败不会覆盖当前配置。</span>
        </div>
        <div class="hint">配置包含私有订阅地址和凭据，请勿截图或直接分享。</div><div class="config-state" id="sr-config-state" role="status">配置已保存</div><textarea id="sr-config-text" aria-label="高级配置" spellcheck="false" oninput="configEdited()"></textarea>
        <div class="card" style="margin-top:12px">
          <h3>软件更新</h3>
          <div class="toolbar">
            <button class="btn btn-sm" onclick="srCheckUpdate()">检查更新</button>
            <button class="btn btn-success btn-sm" id="sr-btn-apply-update" style="display:none" onclick="srApplyUpdate()">下载并安装</button>
            <a class="btn btn-sm" id="sr-btn-update-link" href="https://github.com/sockrockets/sockrocket/releases" target="_blank" rel="noopener">发布记录</a>
          </div>
          <div class="hint" id="sr-update-status">检查 GitHub 发布版本，更新完整插件（程序、页面及脚本）； <strong>config.yaml</strong>、订阅、节点和规则会保留。</div>
        </div>
      </div>

      <!-- ── Page: Logs ────────────────────────────────────────────── -->
      <div class="page" id="sr-page-logs" role="tabpanel" aria-labelledby="sr-nav-logs">
        <div class="page-title">诊断</div>
        <p class="page-intro">先定位问题，再决定是否恢复。检查结果仅代表路由器侧状态，手机等设备仍需实际验证。</p>
        <div class="toolbar"><button class="btn btn-accent" id="sr-diag-btn" onclick="runDiagnostics()">运行检查</button><button class="btn" onclick="restoreDirect()">停止代理并恢复直连</button></div>
        <div class="card" id="sr-diag-card">
          <h3>诊断</h3>
          <div id="sr-diag-body"><div class="hint">点击“运行检查”查看服务、配置、防火墙和 DNS 状态。诊断不会自动重启服务。</div></div>
        </div>
<div class="card"><h3>网络工具</h3><div class="toolbar"><button class="btn" onclick="runSpeedtest()">节点测速</button><button class="btn" onclick="runIpCheck()">检查出口 IP</button></div><div class="stat-grid">          <div class="stat">
            <div class="label">节点延迟</div>
            <div class="value acc" id="sr-st-lat">—</div>
            <div class="sub" id="sr-st-lat-sub">点击测速后显示</div>
          </div>          <div class="stat">
            <div class="label">系统负载</div>
            <div class="value" id="sr-st-load">—</div>
            <div class="sub" id="sr-st-uptime">—</div>
          </div>          <div class="stat">
            <div class="label">出口 IP / 地区</div>
            <div class="value" id="sr-st-ip" style="font-size:15px">—</div>
            <div class="sub" id="sr-st-ip-sub">主动检查后显示</div>
          </div></div></div>
        <h3 class="section-heading">近期日志</h3>
        <div class="toolbar">
          <select id="sr-log-filter" aria-label="日志级别" onchange="srRenderLog()"><option value="all">全部日志</option><option value="problems">警告与错误</option><option value="errors">仅错误</option></select><label class="hint"><input type="checkbox" id="sr-log-hide-reset" onchange="srRenderLog()"> 隐藏远端重置</label><button class="btn btn-sm" onclick="loadLog()">刷新</button>
          <button class="btn btn-danger btn-sm" onclick="srClearLog()">清空</button>
          <label class="hint" style="display:flex;align-items:center;gap:4px;cursor:pointer">
            <input type="checkbox" id="sr-log-auto" onchange="toggleLogAuto()"> 每 5 秒刷新
          </label>
        </div>
        <div id="sr-log-view">正在加载…</div>
      </div>
    </main>
  </div>

  <!-- ── Status bar ────────────────────────────────────────────────── -->
  <div id="sr-statusbar">
    <div class="sb-left">
      <span id="sr-sb-dot">●</span>
      <span id="sr-sb-state">已停止</span>
      <span id="sr-sb-node" style="overflow:hidden;text-overflow:ellipsis;max-width:280px"></span>
    </div>
    <div class="sb-right">
      <span id="sr-sb-ports">SOCKS :1080 · HTTP :1087</span>
      <span id="sr-sb-uptime"></span>
      <span id="sr-sb-nodes">0 个节点</span>
      <span id="sr-sb-ver"></span>
    </div>
  </div>
</div>
<div id="sr-msg" role="status" aria-live="polite"></div>
<dialog id="sr-node-dialog" aria-labelledby="sr-node-dialog-title"><h2 id="sr-node-dialog-title">节点详情</h2><p class="hint">查看详情不会改变当前连接。</p><dl><dt>名称</dt><dd id="sr-detail-name"></dd><dt>协议</dt><dd id="sr-detail-protocol"></dd><dt>服务器地址</dt><dd id="sr-detail-endpoint">已隐藏</dd><dt>延迟</dt><dd id="sr-detail-latency"></dd></dl><label class="hint"><input type="checkbox" id="sr-detail-reveal" onchange="revealNodeEndpoint()"> 显示私有服务器地址</label><div class="dialog-actions"><button class="btn" onclick="srElement('node-dialog').close()">关闭</button><button class="btn btn-accent" id="sr-detail-connect" onclick="useInspectedNode()">使用该节点</button></div></dialog>

<dialog id="sr-sub-dialog" aria-labelledby="sr-sub-dialog-title"><h2 id="sr-sub-dialog-title">订阅详情</h2><dl><dt>名称</dt><dd id="sr-sub-detail-name"></dd><dt>格式</dt><dd id="sr-sub-detail-format"></dd><dt>地址</dt><dd id="sr-sub-detail-url">已隐藏</dd></dl><label class="hint"><input type="checkbox" id="sr-sub-detail-reveal" onchange="revealSubscriptionUrl()"> 显示私有订阅地址</label><p class="hint">地址可能包含访问令牌，请勿分享截图。</p><div class="dialog-actions"><button class="btn" onclick="srElement('sub-dialog').close()">关闭</button></div></dialog>


<dialog id="sr-rule-dialog" aria-labelledby="sr-rule-dialog-title"><h2 id="sr-rule-dialog-title">规则详情</h2><pre id="sr-rule-detail-text" class="rule-preview"></pre><div class="dialog-actions"><button class="btn" onclick="srElement('rule-dialog').close()">关闭</button></div></dialog>
<dialog id="sr-rule-preview-dialog" aria-labelledby="sr-rule-preview-title"><h2 id="sr-rule-preview-title">确认规则更改</h2><p class="hint" id="sr-rule-preview-summary"></p><pre id="sr-rule-preview-text" class="rule-preview"></pre><p class="hint">确认后由路由器校验并保存；任一行格式有误将不保存。运行中的服务会重启以应用规则，可能短暂影响连接。</p><div class="dialog-actions"><button class="btn" onclick="srElement('rule-preview-dialog').close()">返回编辑</button><button class="btn btn-accent" id="sr-rule-preview-apply" onclick="applyRulePreview()">确认应用</button></div></dialog>

<dialog id="sr-rule-export-dialog" aria-labelledby="sr-rule-export-title"><h2 id="sr-rule-export-title">导出规则</h2><p class="hint" id="sr-rule-export-count"></p><textarea id="sr-rule-export-text" readonly aria-label="导出的规则" spellcheck="false"></textarea><div class="dialog-actions"><button class="btn" onclick="srElement('rule-export-dialog').close()">关闭</button><button class="btn btn-accent" onclick="srCopyText(srElement('rule-export-text').value)">复制规则</button></div></dialog>

<script>
/* ══ API ═══════════════════════════════════════════════════════════ */
/* Firmware transports, auto-detected at boot. The built-in HTTP API is
 * loopback-only and token-protected; a browser cannot connect to it directly.
 * The legacy 'http' transport branch below is retained for older pages.
 *   'cgi'  — legacy: direct CGI at /cgi-bin/sockrocket.cgi (the stock Merlin
 *            httpd does NOT execute user CGIs; kept for old installs)
 *   'ksc'  — koolcenter software center (read-only /www): POST /_api/
 *            dispatches sockrocket_api.sh, result polled from /_temp/*.json */
function srElement(id) { return document.getElementById('sr-' + id); }

function sockrocketInitHost() {
  document.form.current_page.value = location.pathname.split('/').pop() || 'Module_sockrocket.asp';
  document.form.next_page.value = document.form.current_page.value;
  if (document.form.preferred_lang) document.form.preferred_lang.value = typeof ui_lang === 'string' ? ui_lang : 'EN';
  if (document.form.firmver) document.form.firmver.value = typeof exist_firmver === 'string' ? exist_firmver : '';
  try {
    if (typeof show_menu !== 'function') throw new Error('Firmware navigation unavailable');
    show_menu();
  } catch (e) {
    document.getElementById('sr-host-notice').hidden = false;
  }
}

var TRANSPORT = null;
/* Port comes from api.js (generated by sockrocket.sh from config.yaml's api_port);
 * 18188 is the built-in default matching sockrocket-cli's DEFAULT_API_PORT. */
var API_PORT = (typeof SOCKROCKET_API_PORT === 'number' && SOCKROCKET_API_PORT > 0) ? SOCKROCKET_API_PORT : 18188;

function apiBase() { return 'http://' + location.hostname + ':' + API_PORT + '/api'; }

function probeTransport(cb) {
  probeGet('/cgi-bin/sockrocket.cgi?action=version', 2500, function (ok) {
    TRANSPORT = ok ? 'cgi' : 'ksc';
    cb();
  });
}

function probeGet(url, timeout, cb) {
  var xhr = new XMLHttpRequest();
  var done = false;
  function finish(ok) { if (!done) { done = true; cb(ok); } }
  // xhr.open() throws on a malformed URL, and the handlers below were already
  // installed by then — the throw escaped as an uncaught pageerror and killed
  // the whole boot (every button dead). Keep it inside the try.
  try {
    xhr.open('GET', url, true);
    xhr.setRequestHeader('X-Sockrocket-Request', '1');
    if (timeout) xhr.timeout = timeout;
    xhr.onload = function () { finish(xhr.status === 200 && xhr.responseText.indexOf('"version"') >= 0); };
    xhr.onerror = function () { finish(false); };
    xhr.ontimeout = function () { finish(false); };
    xhr.send(null);
  } catch (e) { finish(false); }
}

function srApi(action, data, cb, errCb, timeoutMs) {
  var deadline = Date.now() + (timeoutMs || 15000);
  function attempt() {
    var remaining = deadline - Date.now();
    if (remaining <= 0) { if (errCb) errCb({error_code: 'timeout'}); return; }
    function failed(error) {
      // A busy response means the backend has not run this request.
      // Keep its queue slot and retry rather than recording a failed node.
      if (error && error.error_code === 'busy') {
        setTimeout(attempt, 300 + Math.floor(Math.random() * 300));
      } else if (errCb) errCb(error);
    }
    if (TRANSPORT === 'ksc') return apiKsc(action, data, cb, failed, remaining);
    return apiXhr(TRANSPORT === 'http' ? apiBase() : '/cgi-bin/sockrocket.cgi', action, data, cb, failed, remaining);
  }
  return attempt();
}

function apiXhr(base, action, data, cb, errCb, timeoutMs) {
  var xhr = new XMLHttpRequest();
  // Same as probeGet: open()/setRequestHeader() throw on a malformed URL and
  // must not escape uncaught.
  try {
    xhr.open('POST', base + '?action=' + action, true);
    xhr.setRequestHeader('X-Sockrocket-Request', '1');
    xhr.setRequestHeader('Content-Type', 'application/json');
    xhr.timeout = timeoutMs || 15000;
  } catch (e) {
    srShowMsg('无法连接 Sockrocket 接口服务', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
    return;
  }
  xhr.onload = function () {
    if (xhr.status < 200 || xhr.status >= 300) { srShowMsg('接口请求失败（' + xhr.status + '）','err'); if (errCb) errCb({error_code:'http_error'}); return; }
    try {
      var d = JSON.parse(xhr.responseText);
      if (d.error_code || d.error) {
        if (d.error_code !== 'busy') srShowMsg('错误：' + (d.error_message || d.error || d.error_code), 'err');
        if (errCb) errCb(d);
      } else { cb(d); }
    } catch (e) {
      srShowMsg('接口返回内容无法解析', 'err');
      if (errCb) errCb({ error_code: 'parse_error' });
    }
  };
  xhr.onerror = function () {
    srShowMsg('无法连接 Sockrocket 接口服务', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
  };
  xhr.ontimeout = function () {
    srShowMsg('接口请求超时，请稍后重试。','err');
    if (errCb) errCb({ error_code: 'timeout' });
  };
  try { xhr.send(JSON.stringify(data || {})); } catch (e) { srShowMsg('无法发送接口请求。','err'); if (errCb) errCb({error_code:'network_error'}); }
}

var kscQueue = [];
var kscBusy = false;
function apiKsc(action, data, cb, errCb, timeoutMs) {
  // Stop must remain usable while a subscription fetch is queued/running.
  if (action === 'stop' || action === 'speedtest_node') return apiKscRequest(action, data, cb, errCb, timeoutMs);
  if (kscQueue.length >= 16) {
    if (errCb) errCb({ error_code: 'busy' });
    return;
  }
  kscQueue.push([action, data, cb, errCb, timeoutMs]);
  drainKscQueue();
}
function drainKscQueue() {
  if (kscBusy || !kscQueue.length) return;
  kscBusy = true;
  var request = kscQueue.shift();
  function finish(callback, value) {
    kscBusy = false;
    try { if (callback) callback(value); }
    finally { drainKscQueue(); }
  }
  apiKscRequest(request[0], request[1], function (d) { finish(request[2], d); },
    function (e) { finish(request[3], e); }, request[4]);
}
function apiKscRequest(action, data, cb, errCb, timeoutMs) {
  // Full ms timestamp + wide random suffix: the old `Date.now() % 1e9 +
  // random(0..999)` form collided when two requests fired within the same
  // millisecond, making them poll (and possibly read) the same result file.
  var random = new Uint8Array(16);
  try { window.crypto.getRandomValues(random); }
  catch (e) {
    srShowMsg('请使用支持安全随机数的现代浏览器', 'err');
    if (errCb) errCb({ error_code: 'unsupported_browser' });
    return;
  }
  var seq = Array.prototype.map.call(random, function (b) {
    return ('0' + b.toString(16)).slice(-2);
  }).join('');
  var fields = {};
  // Softcenter may shell-escape JSON quotes stored in fields. Encode UTF-8
  // before dispatch so Chinese names, URLs and rule text arrive byte-for-byte.
  fields['sockrocket_rpc_b64_' + seq] = utf8b64(JSON.stringify(data || {}));
  var job = {
    id: Math.floor(Math.random() * 999999) + 1,
    method: 'sockrocket_api.sh',
    params: [action, seq],
    fields: fields
  };
  var xhr = new XMLHttpRequest();
  xhr.open('POST', '/_api/', true);
  xhr.setRequestHeader('X-Sockrocket-Request', '1');
  xhr.setRequestHeader('Content-Type', 'application/json');
  xhr.timeout = timeoutMs || 45000;
  xhr.onload = function () {
    if (xhr.status !== 200) {
      srShowMsg('路由器接口请求失败（' + xhr.status + ')', 'err');
      if (errCb) errCb({ error_code: 'http_error' });
      return;
    }
    try {
      var ack = JSON.parse(xhr.responseText);
      if (String(ack.result) !== String(job.id)) throw new Error('dispatch rejected');
    } catch (e) {
      srShowMsg('路由器拒绝了接口请求', 'err');
      if (errCb) errCb({ error_code: 'dispatch_error' });
      return;
    }
    pollKscResult(seq, 0, cb, errCb, Date.now() + (timeoutMs || 80000));
  };
  xhr.ontimeout = function () {
    srShowMsg('路由器接口请求超时', 'err');
    if (errCb) errCb({ error_code: 'timeout' });
  };
  xhr.onerror = function () {
    srShowMsg('无法连接路由器接口', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
  };
  xhr.send(JSON.stringify(job));
}

function pollKscResult(seq, tries, cb, errCb, deadline) {
  // Network timeouts count toward the result budget, not only retry delays.
  if (tries > 400 || Date.now() >= deadline) {
    srShowMsg('请求超时', 'err');
    if (errCb) errCb({ error_code: 'timeout' });
    return;
  }
  var xhr = new XMLHttpRequest();
  xhr.open('GET', '/_temp/sockrocket_rpc_' + seq + '.json?_=' + Math.random(), true);
  xhr.timeout = 5000;
  xhr.onload = function () {
    var t = (xhr.responseText || '').trim();
    if (xhr.status === 200 && t.charAt(0) === '{') {
      try {
        var d = JSON.parse(t);
        acknowledgeKscResult(seq);
        if (d.error_code || d.error) {
          if (d.error_code !== 'busy') srShowMsg('错误：' + (d.error_message || d.error || d.error_code), 'err');
          if (errCb) errCb(d);
        } else { cb(d); }
        return;
      } catch (e) { /* fall through to retry */ }
    }
    setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb, deadline); }, 200);
  };
  xhr.onerror = function () {
    setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb, deadline); }, 200);
  };
  xhr.ontimeout = xhr.onerror;
  try { xhr.send(null); } catch (e) { setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb, deadline); }, 200); }
}


function acknowledgeKscResult(seq) {
  // Cleanup failures fall back to server expiry and must not hide the answer.
  try {
    var xhr = new XMLHttpRequest();
    xhr.open('POST', '/_api/', true);
    xhr.setRequestHeader('Content-Type', 'application/json');
    xhr.timeout = 5000;
    xhr.send(JSON.stringify({ id: Math.floor(Math.random() * 999999) + 1,
      method: 'sockrocket_api.sh', params: ['rpc_ack', seq], fields: {} }));
  } catch (e) { /* server expiry remains active */ }
}

function b64utf8(b64) {
  try {
    var bin = atob(b64 || '');
    var bytes = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new TextDecoder('utf-8').decode(bytes);
  } catch (e) { return ''; }
}
function utf8b64(s) {
  var bytes = new TextEncoder().encode(s);
  var bin = '';
  for (var i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin);
}
function srEsc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
    return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
  });
}
function latClass(ms) { return ms < 0 ? 'bad' : ms < 200 ? 'good' : ms < 500 ? 'mid' : 'bad'; }
function latText(ms) { return ms < 0 ? '超时' : ms + 'ms'; }

var srMsgTimer;
function srShowMsg(msg, type) {
  clearTimeout(srMsgTimer);
  var el = srElement('msg');
  el.textContent = msg;
  el.className = type || 'info';
  el.style.display = 'block';
  srMsgTimer = setTimeout(function () { el.style.display = 'none'; }, type === 'err' ? 8000 : 4000);
}
function toastResp(d, fallback) {
  if (d.ok === false) srShowMsg(d.msg || '操作失败', 'err');
  else srShowMsg(d.msg || fallback || '已完成', 'ok');
}
/** Copy plain text to the clipboard (Clipboard API + textarea fallback for older WebUI browsers). */
function srCopyText(text, okMsg) {
  if (!text) { srShowMsg('没有可复制的内容', 'err'); return; }
  function ok() { srShowMsg(okMsg || '已复制', 'ok'); }
  function fail() {
    var ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.cssText = 'position:fixed;left:-9999px;top:0';
    document.body.appendChild(ta);
    ta.select();
    ta.setSelectionRange(0, text.length);
    var worked = false;
    try { worked = document.execCommand('copy'); } catch (e) {}
    document.body.removeChild(ta);
    if (worked) ok();
    else srShowMsg('复制失败，请手动选择地址复制', 'err');
  }
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(ok, fail);
  } else {
    fail();
  }
}

/* ══ State ═════════════════════════════════════════════════════════ */
var SR_STATE = {
  page: 'home',
  statusKnown: false,
  statusAt: 0,
  status: {},        // status action result
  stats: {},         // stats action result
  nodes: [],         // node_details
  subs: [],          // subscriptions (for copy-url by index)
  activeNode: 0,
  lat: {},           // index -> ms (-1 = timeout)
  testingAll: false,
};
var SR_PAGES = ['home', 'nodes', 'subs', 'dns', 'network', 'logs', 'config'];
var SR_PAGE_LOADED = {};

function srShowPage(p) {
  if (SR_PAGES.indexOf(p) < 0) return;
  SR_STATE.page = p;
  SR_PAGES.forEach(function (id) {
    srElement('page-' + id).classList.toggle('active', id === p);
    srElement('nav-' + id).classList.toggle('active', id === p);
    srElement('nav-' + id).setAttribute('aria-selected', id === p ? 'true' : 'false');
    srElement('nav-' + id).tabIndex = id === p ? 0 : -1;
  });
  if (!SR_PAGE_LOADED[p]) {
    SR_PAGE_LOADED[p] = true;
    if (p === 'nodes') { srLoadNodes(); loadProxyGroups(); }
    if (p === 'subs') srLoadSubs();
    if (p === 'dns') { srLoadRules(); ruleTypeChanged(); }
    if (p === 'config') srLoadConfig();
    if (p === 'network') loadNetworkPolicy();
    if (p === 'logs') loadLog();
  } else {
    if (p === 'nodes') renderNodes();
  }
}

/* Structured DNS policy; stale reads never overwrite an edited form. */
var dnsPolicyRevision='',dnsPolicyPending=false,dnsPolicyDirty=false,dnsPolicyGeneration=0,dnsTestGeneration=0,dnsTestPending=false;
var DNS_POLICY_FIELDS=['ipv6-mode','dns-strategy','dns-direct','dns-proxy','dns-bootstrap','dns-cache','dns-capacity','dns-ttl','dns-timeout','dns-domains'];
function networkStateText(d){
  if(!d.running)return '服务已停止；显示已保存配置';
  if(d.ipv6_active)return 'IPv6 双栈已生效；LAN 可达性需实测';
  if(d.ipv6_mode==='dual_stack')return '已选双栈，但本机尚未完成 IPv6 接管（常见原因：固件未开 IPv6 或没有 LAN 前缀）。IPv4 代理保持原样，可改回兼容模式';
  return '当前为 IPv4 兼容模式（推荐，不依赖运营商 IPv6）';
}
function networkPolicyEdited(){dnsPolicyDirty=true;dnsPolicyGeneration++;dnsTestGeneration++;srElement('dns-test-result').textContent='配置已修改，请重新测试。';srElement('network-state').textContent='有未保存的修改';}
function lockDnsPolicy(on){dnsPolicyPending=on;var busy=on||networkPending;DNS_POLICY_FIELDS.forEach(function(id){srElement(id).disabled=busy;});['network-save','network-reload','dns-test'].forEach(function(id){srElement(id).disabled=busy||(id==='dns-test'&&dnsTestPending)||(!dnsPolicyRevision&&id!=='network-reload');});}
function dnsPolicyBody(){
  function lines(id){return srElement(id).value.split(/\r?\n/).map(function(s){return s.trim();}).filter(Boolean);}
  function number(id,min,max){var text=srElement(id).value.trim(),n=Number(text);if(!text||!Number.isInteger(n)||n<min||n>max)throw new Error('请核对缓存条数、TTL 和查询超时的范围。');return n;}
  return {dns:{direct_servers:lines('dns-direct'),proxy_servers:lines('dns-proxy'),bootstrap_servers:lines('dns-bootstrap'),direct_domains:lines('dns-domains'),strategy:srElement('dns-strategy').value,cache_enabled:srElement('dns-cache').value==='true',cache_capacity:number('dns-capacity',16,16384),cache_ttl:number('dns-ttl',0,86400),timeout_secs:number('dns-timeout',1,12)},ipv6_mode:srElement('ipv6-mode').value,revision:dnsPolicyRevision};
}
function loadNetworkPolicy(done){
  if(dnsPolicyPending)return;
  var generation=++dnsPolicyGeneration;lockDnsPolicy(true);
  srApi('network_settings',null,function(d){
    if(generation!==dnsPolicyGeneration){lockDnsPolicy(false);if(done)done(false);return;}
    if(!d.ok||!d.dns||!d.revision){lockDnsPolicy(false);srElement('network-state').textContent=d.msg||'网络配置读取失败';if(done)done(false);return;}
    var s=d.dns;dnsPolicyRevision=d.revision;dnsPolicyDirty=false;
    ['direct','proxy','bootstrap'].forEach(function(k){srElement('dns-'+k).value=s[k+'_servers'].join('\n');});
    srElement('dns-domains').value=s.direct_domains.join('\n');srElement('dns-strategy').value=s.strategy;srElement('ipv6-mode').value=d.ipv6_mode;srElement('dns-cache').value=String(s.cache_enabled);srElement('dns-capacity').value=s.cache_capacity;srElement('dns-ttl').value=s.cache_ttl;srElement('dns-timeout').value=s.timeout_secs;
    srElement('network-state').textContent=networkStateText(d);lockDnsPolicy(false);if(done)done(true);
  },function(){lockDnsPolicy(false);srElement('network-state').textContent='网络配置读取失败，请重试';if(done)done(false);});
}
function saveNetworkPolicy(){
  if(dnsPolicyPending||networkControlsBusy()||!dnsPolicyRevision)return;
  var body;try{body=dnsPolicyBody();}catch(e){srShowMsg(e.message,'err');return;}
  lockDnsPolicy(true);networkPending=true;statusGeneration++;dnsTestGeneration++;renderChrome();
  srElement('network-state').textContent='正在保存并应用网络策略…';
  function finish(d,failed){
    lockDnsPolicy(false);
    if(!failed && !d.ok && !d.saved){networkPending=false;lockDnsPolicy(false);renderChrome();srElement('network-state').textContent=d.msg||'保存失败，表单修改已保留';srShowMsg(d.msg||'保存失败，请检查表单。','err');return;}
    // Refresh both saved policy and runtime status before releasing mutation controls.
    loadNetworkPolicy(function(ok){
      var generation=++statusGeneration;
      srApi('status',null,function(st){networkPending=false;lockDnsPolicy(false);if(generation===statusGeneration)acceptStatus(st);else renderChrome();if(failed||!d.ok)srShowMsg(d.msg||'请求未完成，请核对已保存配置与实际运行状态。','err');else toastResp(d);},function(){networkPending=false;lockDnsPolicy(false);SR_STATE.statusKnown=false;renderChrome();srShowMsg('应用后状态未确认，请检查诊断。','err');});
      if(!ok)srElement('network-state').textContent='应用结果尚未确认，请重新加载。';
    });
  }
  srApi('save_network_settings',body,function(d){finish(d,false);},function(d){finish(d||{},true);},60000);
}
function testDnsPolicy(){
  if(dnsPolicyPending||dnsTestPending||networkPending||!dnsPolicyRevision)return;
  var body;try{body=dnsPolicyBody();}catch(e){srShowMsg(e.message,'err');return;}
  body.domain=srElement('dns-test-domain').value.trim();body.group=srElement('dns-test-group').value;body.qtype=srElement('dns-test-type').value;
  dnsTestPending=true;var generation=++dnsTestGeneration;srElement('dns-test').disabled=true;srElement('dns-test-result').textContent='正在查询…';
  function finish(d){d=d||{};dnsTestPending=false;srElement('dns-test').disabled=dnsPolicyPending||networkPending;if(generation!==dnsTestGeneration)return;srElement('dns-test-result').textContent=d.ok?body.group+' / '+body.qtype+' · '+d.elapsed_ms+' ms · '+(Array.isArray(d.addresses)&&d.addresses.length?d.addresses.join(', '):'成功返回，无此类型记录'):(d.error_code||'dns_test_failed')+' · '+(d.msg||'查询失败，请检查上游和网络。');}
  srApi('dns_test',body,finish,finish,20000);
}

/* ══ Title bar / status bar / home ═════════════════════════════════ */
var refreshPending = {}, statusGeneration = 0, networkPending = false;
function refreshStatus(withSysinfo) {
  var generation=statusGeneration;
  function read(action, success) {
    if (refreshPending[action]) return;
    refreshPending[action] = true;
    srApi(action, null, function (d) { refreshPending[action] = false; success(d); }, function () {
      refreshPending[action] = false;
      if (action === 'status' && generation===statusGeneration) {
        SR_STATE.statusKnown = false;
        srElement('status-freshness').textContent = '状态读取失败，显示的是上次结果';
        renderChrome();
      }
    });
  }
  read('status', function (d) {
    if(generation!==statusGeneration){refreshStatus(withSysinfo);return;}
    acceptStatus(d);
  });
  read('stats', function (d) { SR_STATE.stats = d; renderChrome(); });
  if (withSysinfo) read('sysinfo', function (d) { SR_STATE.sysinfo = d; renderChrome(); });
}

function acceptStatus(d) {
    if (!d || typeof d.running !== 'boolean') { SR_STATE.statusKnown = false; renderChrome(); return; }
    SR_STATE.status = d; SR_STATE.statusKnown = true; SR_STATE.statusAt = Date.now();
    srElement('status-freshness').textContent = '更新于 ' + new Date().toLocaleTimeString();
    renderChrome(); renderNodes();
    if(SR_STATE.page==='network' && !dnsPolicyPending && !networkPending && !dnsPolicyDirty && d.network_revision && dnsPolicyRevision && d.network_revision!==dnsPolicyRevision)loadNetworkPolicy();
    if(SR_STATE.page==='network' && !dnsPolicyDirty && !dnsPolicyPending && !networkPending)srElement('network-state').textContent=networkStateText(d);
    if(SR_STATE.page==='dns' && !rulePending && !RULE_READ_PENDING && !RULE_FLOW_ERROR && RULE_RULE_REVISION && d.rules_revision && d.rules_revision!==RULE_RULE_REVISION)srLoadRules();
}

function formatTrafficBytes(n) {
  n = Number(n) || 0;
  if (n < 1024) return n + ' B';
  var units = ['KB', 'MB', 'GB', 'TB'], i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < units.length - 1);
  return (n >= 100 ? n.toFixed(0) : n.toFixed(1)) + ' ' + units[i];
}
function trafficName(row) {
  return row && row.id === 'direct' ? '直连' : ((row && row.name) || '未命名');
}
function renderMonthTable(el, keys, rows) {
  if (!el) return;
  keys = keys || [];
  rows = (rows || []).filter(function (row) { return Number(row.total || 0) > 0; }).slice(0, 20);
  if (!rows.length) { el.innerHTML = '<div class="hint">暂无用量记录</div>'; return; }
  var head = '<tr><th>名称</th>' + keys.map(function (key) { return '<th>' + srEsc(String(key).slice(5)) + '</th>'; }).join('') + '<th>合计</th></tr>';
  var body = rows.map(function (row) {
    var byKey = {};
    (row.months || []).forEach(function (month) { byKey[month.key] = month; });
    var cells = keys.map(function (key) {
      var month = byKey[key] || {};
      var total = Number(month.total || 0);
      var title = '上行 ' + formatTrafficBytes(month.up) + ' · 下行 ' + formatTrafficBytes(month.down);
      return '<td title="' + srEsc(title) + '">' + (total ? formatTrafficBytes(total) : '—') + '</td>';
    }).join('');
    return '<tr><td title="' + srEsc(trafficName(row)) + '">' + srEsc(trafficName(row)) + '</td>' + cells + '<td>' + formatTrafficBytes(row.total) + '</td></tr>';
  }).join('');
  el.innerHTML = '<table class="traffic-table"><thead>' + head + '</thead><tbody>' + body + '</tbody></table>';
}
function renderSessionTraffic(traffic) {
  var el = srElement('traffic-list');
  if (!el) return;
  var rows = ((traffic && traffic.nodes) || []).filter(function (row) { return Number(row.session_total || 0) > 0; }).slice(0, 20);
  if (!rows.length) { el.innerHTML = '<div class="hint">暂无流量记录</div>'; return; }
  el.innerHTML = '<table class="traffic-table"><thead><tr><th>节点</th><th>上行</th><th>下行</th><th>合计</th></tr></thead><tbody>' + rows.map(function (row) {
    return '<tr><td title="' + srEsc(trafficName(row)) + '">' + srEsc(trafficName(row)) + '</td><td>' + formatTrafficBytes(row.session_up) + '</td><td>' + formatTrafficBytes(row.session_down) + '</td><td>' + formatTrafficBytes(row.session_total) + '</td></tr>';
  }).join('') + '</tbody></table>';
}
function renderSubscriptionTraffic(traffic) {
  renderMonthTable(srElement('sub-traffic-months'), traffic && traffic.recent_month_keys, traffic && traffic.recent_months);
  renderMonthTable(srElement('sub-link-traffic'), traffic && traffic.recent_month_keys, traffic && traffic.recent_subscriptions);
}
function renderChrome() {
  var st = SR_STATE.status, ss = SR_STATE.stats, si = SR_STATE.sysinfo || {};
  renderSessionTraffic(ss && ss.traffic);
  renderSubscriptionTraffic(ss && ss.traffic);
  var running = !!st.running;
  var pill = srElement('service-pill');
  pill.textContent = !SR_STATE.statusKnown ? '状态未知' : running ? '运行中' : '已停止';
  pill.className = 'service-pill' + (SR_STATE.statusKnown && running ? ' success' : '');

  srElement('health-process').textContent = !SR_STATE.statusKnown ? '状态未知' : running ? '运行正常' : '已停止';
  var probe = st.health || {}, age = Date.now() / 1000 - Number(probe.last_probe_at || 0);
  srElement('health-outbound').textContent = !SR_STATE.statusKnown ? '状态未知' : !running ? '未运行' : age >= 0 && age < 120
    ? (probe.consecutive_failures > 0 ? '最近探测失败' : '最近检查通过 · ' + Math.round(age) + ' 秒前') : '近期没有探测结果';
  var applyLabels = {applied:'已生效',validating:'正在校验',rejected:'更新被拒绝，沿用原配置',restart_required:'部分设置待重启',policy_pending:'正在应用网络规则',policy_failed:'网络规则应用失败'};
  srElement('config-apply').textContent = !SR_STATE.statusKnown ? '状态未知' : !running ? '待启动' : applyLabels[(probe.config_apply || {}).state] || '待确认';
  var runtime = probe.runtime || {}, tun = runtime.tun || {};
  var active = tun.running && typeof tun.active === 'number' ? tun.active : runtime.active_connections;
  srElement('st-connections').textContent = running && typeof active === 'number' ? String(active) : '—';
  srElement('st-connections-sub').textContent = !running ? '服务未运行' : typeof active !== 'number' ? '当前版本未提供计数' : tun.running ? 'TUN 会话 · 约每 30 秒更新' : '代理连接 · 约每 30 秒更新';
  srElement('st-probe').textContent = running && age >= 0 && age < 120 && probe.last_probe_ms != null ? probe.last_probe_ms + ' ms' : '—';
  srElement('st-probe-sub').textContent = !running ? '服务未运行' : probe.consecutive_failures > 0 ? '最近探测失败' : age >= 0 && age < 120 ? '最近检查通过' : '近期没有探测结果';
  srElement('st-service-uptime').textContent = running && ss.uptime && ss.uptime !== 'stopped' ? ss.uptime : '—';
  // title bar
  var nbName = srElement('nb-name');
  nbName.textContent = SR_STATE.nodes[st.active_node] ? SR_STATE.nodes[st.active_node].name : (st.active_node != null ? '节点 #' + st.active_node : '—');
  var activeLat = SR_STATE.lat[st.active_node];
  var nbLat = srElement('nb-lat');
  nbLat.textContent = activeLat != null ? latText(activeLat) : '';
  nbLat.className = 'nb-lat' + (activeLat != null && activeLat < 0 ? ' bad' : '');

  // status bar
  var dot = srElement('sb-dot');
  dot.style.color = running ? 'var(--success)' : 'var(--text-muted)';
  srElement('sb-state').textContent = !SR_STATE.statusKnown ? '状态未知' : running ? '运行中' : '已停止';
  srElement('sb-state').className = running ? 'ok' : '';
  srElement('sb-node').textContent =
    (SR_STATE.nodes[st.active_node] ? SR_STATE.nodes[st.active_node].name : '') +
    (activeLat != null ? ' · ' + latText(activeLat) : '');
  srElement('sb-ports').textContent = 'SOCKS :' + (st.socks_port || 1080) + ' · HTTP :' + (st.http_port || 1087);
  srElement('sb-uptime').textContent = ss.uptime && ss.uptime !== 'stopped' ? '↑ ' + ss.uptime : '';
  srElement('sb-nodes').textContent = (st.node_count || 0) + ' 个节点';
  srElement('sb-ver').textContent = st.version || '';

  // home hero
  var hi = srElement('hero-icon');
  hi.className = 'hero-icon' + (running ? '' : ' off');
  srElement('hero-state').textContent = !SR_STATE.statusKnown ? '正在确认服务状态' : running ? '代理服务正在运行' : '代理服务已停止';
  srElement('hero-state').style.color = SR_STATE.statusKnown && running ? 'var(--success)' : 'var(--text-secondary)';
  srElement('hero-live').style.display = running ? '' : 'none';
  var n = SR_STATE.nodes[st.active_node];
  srElement('hero-node').textContent = n
    ? n.name + ' · ' + (n.protocol || '?') + ' · ' + (n.port || '?')
    : (st.node_count > 0 ? '节点 #' + st.active_node : '暂无节点，请先添加订阅或节点');
  srElement('hero-uptime').textContent = running ? (ss.uptime || '—') : '—';
  // health check line: probe latency / failure streak / failover history
  var hh = srElement('hero-health');
  var health = st.health;
  if (running && health && health.current_node_name) {
    var hp = ['最近探测 ' + (health.last_probe_ms != null ? health.last_probe_ms + 'ms' : '—')];
    if (health.consecutive_failures > 0) hp.push(health.consecutive_failures + ' 次连续失败');
    if (health.switch_count > 0 && health.last_switch) {
      var swd = new Date(health.last_switch.at * 1000);
      var swt = ('0' + swd.getHours()).slice(-2) + ':' + ('0' + swd.getMinutes()).slice(-2);
      hp.push('自动切换 ' + health.switch_count + ' 次（最近 → ' + health.last_switch.node_name + ' ' + swt + ')');
    } else {
      hp.push('尚未自动切换');
    }
    hh.textContent = '⚡ ' + hp.join(' · ');
    hh.style.color = health.consecutive_failures > 0 ? 'var(--warning, orange)' : 'var(--text-muted)';
    hh.style.display = '';
  } else {
    hh.style.display = 'none';
  }
  srElement('hero-nodes').textContent = st.node_count || 0;
  srElement('hero-subs').textContent = st.sub_count || 0;
  var tb = srElement('toggle-btn');
  tb.disabled = servicePending || !SR_STATE.statusKnown || (networkPending && !running);
  tb.textContent = servicePending ? '正在处理…' : !SR_STATE.statusKnown ? '等待状态' : running ? '停止服务' : '启动服务';
  tb.className = 'btn ' + (running ? 'btn-danger' : 'btn-success');
  tb.title=running?'停止服务并清理 DNS 接管与 TUN':'启动服务，默认同时开启 DNS 接管与 TUN';

  renderNetworkControls();
  setProxyModeUi(st.proxy_mode || 'rule');
  if (SR_STATE.page === 'dns') renderRuleFlow();
  srElement('wan-ip').textContent = (si.wan_addresses || []).join(' / ') || si.wan_ip || '—';

  // stat cards
  srElement('st-mem').textContent = si.mem_pct != null ? (si.memory_source === 'estimated' ? '约 ' : '') + si.mem_pct + '%' : '—';
  srElement('st-mem-sub').textContent = si.mem_used_kb != null ? Math.round(si.mem_used_kb / 1024) + ' / ' + Math.round(si.mem_total_kb / 1024) + ' MB' : '';
  srElement('st-load').textContent = si.load || '—';
  srElement('st-uptime').textContent = si.uptime ? '路由器运行 ' + si.uptime : '';
}

function setChip(id, on) {
  var dot = srElement(id + '-dot');
  if (!dot) return;
  dot.className = 'dot ' + (on ? 'ok' : 'off');
  srElement(id + '-state').textContent = on ? '已开启' : '已关闭';
}

// Saved preferences survive Stop; the chips report whether they can be active
// now. Stopping already removes the DNS/iptables/TUN wiring in the controller.
function networkControlsBusy() {
  return !!servicePending || networkPending;
}
function renderNetworkControls() {
  var st=SR_STATE.status||{},online=SR_STATE.statusKnown&&!!st.running,busy=networkControlsBusy();
  ['chip-dns','chip-fw'].forEach(function(id){srElement(id).disabled=!online||busy;srElement(id).classList.toggle('pending',networkPending);});
  setChip('dns',online&&st.dns_hijack);setChip('fw',online&&st.transparent_proxy);
  var cn=srElement('chip-cn'),globalMode=st.proxy_mode==='global';
  cn.disabled=!SR_STATE.statusKnown||busy||globalMode;
  cn.classList.toggle('pending',networkPending);
  setChip('cn',!!st.cn_ipset_direct);
  if(st.cn_ipset_direct&&!(online&&st.transparent_proxy))srElement('cn-state').textContent='已保存，待启用 TUN';
  cn.title=globalMode?'全局模式下国内 IP 也走代理。':'命中国内 IP 的局域网流量会绕过 TUN 和域名分流规则。开启透明代理后生效。';
  if(!SR_STATE.statusKnown){['dns','fw','cn'].forEach(function(id){srElement(id+'-state').textContent='待确认';srElement('chip-'+id).title='状态尚未确认，请等待最新状态。';});return;}
  srElement('chip-dns').title=!online?'服务已停止，DNS 接管已关闭；启动服务时默认开启。':st.dns_hijack?(st.transparent_proxy?'DNS 接管与 TUN 已开启，使用短 TTL Fake-IP':'DNS 接管已开启，使用短 TTL 真实地址'):'DNS 接管已关闭，局域网使用原有 DNS';
  srElement('chip-fw').title=!online?'服务已停止，TUN 透明代理已关闭；启动服务时默认开启。':st.transparent_proxy?'透明代理已开启；单独关闭它会保留 DNS 接管':'透明代理已关闭，DNS 接管由单独开关控制';
}

function applyNetworkToggles(body) {
  networkPending=true;statusGeneration++;renderNetworkControls();
  function reconcile(reply,failed) {
    var generation=++statusGeneration;
    srApi('status',null,function(d){
      networkPending=false;
      if(generation===statusGeneration)acceptStatus(d);else renderChrome();
      if(failed)srShowMsg('设置请求未完成，已刷新最新状态，请核对后再操作。','err');else toastResp(reply);
    },function(){
      networkPending=false;if(generation===statusGeneration)SR_STATE.statusKnown=false;renderChrome();
      srShowMsg('设置后的状态读取失败，请等待刷新；不要重复点击。','err');
    });
  }
  srApi('set_toggles',body,function(d){reconcile(d,false);},function(d){reconcile(d,true);},60000);
}
function toggleCnDirect() {
  if (!SR_STATE.statusKnown || networkControlsBusy() || SR_STATE.status.proxy_mode==='global') return;
  applyNetworkToggles({cn_ipset_direct:!SR_STATE.status.cn_ipset_direct});
}
function toggleDns() {
  if (!SR_STATE.statusKnown || !SR_STATE.status.running || networkControlsBusy()) return;
  applyNetworkToggles({dns_hijack:!SR_STATE.status.dns_hijack});
}
function setProxyModeUi(mode) {
  var m = (mode || 'rule').toLowerCase();
  ['rule', 'global', 'direct'].forEach(function (k) {
    var el = srElement('mode-' + k);
    if (el) el.classList.toggle('on', k === m);
  });
  var seg = srElement('mode-seg');
  if (seg) {
    seg.title = m === 'global'
      ? '全局：全部流量经过当前节点'
      : (m === 'direct'
          ? '直连：流量直接访问'
          : '规则：自定义规则、内置国内直连、最终规则依次匹配');
  }
}
function setProxyMode(mode) {
  if (!SR_STATE.statusKnown || srElement('mode-seg').classList.contains('pending')) return;
  var cur = (SR_STATE.status.proxy_mode || 'rule').toLowerCase();
  if (cur === mode) return;
  var seg = srElement('mode-seg');
  if (seg) seg.classList.add('pending');
  srApi('set_proxy_mode', { proxy_mode: mode }, function (d) {
    if (seg) seg.classList.remove('pending');
    toastResp(d);
    refreshStatus(true);
  }, function () {
    if (seg) seg.classList.remove('pending');
  });
}
function toggleProxy() {
  if (!SR_STATE.statusKnown || !SR_STATE.status.running || networkControlsBusy()) return;
  // A standalone TUN disable preserves the independent DNS choice.
  applyNetworkToggles(SR_STATE.status.transparent_proxy?{transparent_proxy:false}:{transparent_proxy:true,dns_hijack:true});
}

/* ══ Service control ═══════════════════════════════════════════════ */
var servicePending = false, serviceInFlight = 0, serviceStopPending = false, serviceGeneration = 0;
function serviceAction(action) {
  // Stop bypasses the lifecycle gate, just as it bypasses the RPC read queue.
  if (action === 'stop' ? serviceStopPending : (servicePending || networkPending)) return;
  var generation = ++serviceGeneration;
  statusGeneration++;
  serviceInFlight++; servicePending = true;
  if (action === 'stop') serviceStopPending = true;
  renderChrome();
  function done(d) {
    serviceInFlight--; servicePending = serviceInFlight > 0;
    if (action === 'stop') serviceStopPending = false;
    // An older start/restart cannot replace a newer stop's feedback.
    if (generation === serviceGeneration && d) toastResp(d);
    statusGeneration++;renderChrome(); refreshStatus(true);
  }
  srApi(action, {}, done, function () { done(); }, 90000);
}

function toggleService() {
  if (!SR_STATE.statusKnown) { srShowMsg('尚未读取服务状态，请先刷新。', 'err'); return; }
  serviceAction(SR_STATE.status.running ? 'stop' : 'start');
}
function restartService() {
  if (!confirm('重启路由器代理？局域网代理连接会短暂中断，设置和节点会保留。')) return;
  serviceAction('restart');
}

/* ══ Speedtest / IP check / diagnose ═══════════════════════════════ */
function runSpeedtest() {
  srElement('st-lat').textContent = '…';
  srElement('st-lat-sub').textContent = '正在测速…';
  srApi('speedtest', null, function (d) {
    var el = srElement('st-lat');
    if (d.ok && d.latency >= 0) {
      el.textContent = d.latency + 'ms';
      el.className = 'value ' + (d.latency < 200 ? 'ok' : d.latency < 500 ? 'acc' : 'bad');
      srElement('st-lat-sub').textContent = '通过当前节点';
      SR_STATE.lat[SR_STATE.status.active_node] = d.latency;
      renderChrome();
    } else {
      el.textContent = '失败';
      el.className = 'value bad';
      srElement('st-lat-sub').textContent = d.msg || '连接失败';
    }
  });
}
function runIpCheck() {
  srElement('st-ip').textContent = '…';
  srElement('st-ip-sub').textContent = 'Checking…';
  srApi('ip_check', null, function (d) {
    if (d.ok && d.ip) {
      srElement('st-ip').textContent = d.ip;
      srElement('st-ip-sub').textContent = d.country || '';
    } else {
      srElement('st-ip').textContent = '失败';
      srElement('st-ip-sub').textContent = d.msg || '';
    }
  });
}
var diagnosePending = false;
function runDiagnostics() {
  if (diagnosePending) return;
  diagnosePending = true;
  var btn = srElement('diag-btn'); btn.disabled = true;
  srElement('diag-body').textContent = '正在检查…';
  function done() { diagnosePending = false; btn.disabled = false; }
  srApi('diagnose', null, function (d) {
    done();
    if (d.ok === false) { srElement('diag-body').textContent = d.msg || '检查失败，请重试'; return; }
    function row(name, ok, detail) {
      return '<div class="diag-row"><span class="d-name">' + name + '</span><span class="' + (ok == null ? '' : ok ? 'check-ok' : 'check-bad') + '">' + (ok ? '✓ ' : '— ') + srEsc(detail) + '</span></div>';
    }
    var html = row('服务进程', d.running, d.running ? '运行中' : '已停止') +
      row('配置校验', d.config_valid, d.config_valid ? '通过' : '未通过') +
      row('IPv4 标记规则', d.iptables_rules == null ? null : d.iptables_rules > 0, d.iptables_rules == null ? '无法检测规则数量' : Number(d.iptables_rules) + ' 条标记规则') +
      row('DNS 接管', d.dns_rules > 0, d.dns_rules > 0 ? '已启用' : '未发现接管规则') +
      row('dnsmasq', d.dnsmasq_running, d.dnsmasq_running ? '运行中' : '未运行') +
      row('IPv4 透明代理链', d.fw_active, d.fw_active == null ? '无法检测' : d.fw_active ? '已挂载' : '未挂载') +
      row('IPv6 透明代理链', d.fw_ipv6, d.fw_ipv6 == null ? '无法检测' : d.fw_ipv6 ? '已挂载' : '未挂载') +
      '<p class="hint">未启用的功能不一定表示故障，请结合网络设置判断。局域网设备联网需要在设备上确认。</p>';
    srElement('diag-body').innerHTML = html;
  }, function () { done(); srElement('diag-body').textContent = '检查请求失败，请确认路由器管理连接后重试。'; });
}
function restoreDirect() {
  if (!confirm('停止路由器代理并恢复直连？海外网站可能暂时无法访问，订阅、节点和规则会保留。')) return;
  serviceAction('stop');
}

/* ══ Nodes ═════════════════════════════════════════════════════════ */
// Keep measurements and unfinished batch work across reloads in this tab.
var savedSpeedTests = (function () {
  try { return JSON.parse(sessionStorage.getItem('sr-speedtests-v1')) || {}; }
  catch (e) { return {}; }
})();
var speedTestsRestored = false;
function saveSpeedTests() {
  savedSpeedTests.results = SR_STATE.nodes.map(function (n, i) {
    return {key: nodeViewKey(n), latency: SR_STATE.lat[i]};
  }).filter(function (entry) { return typeof entry.latency === 'number'; });
  try { sessionStorage.setItem('sr-speedtests-v1', JSON.stringify(savedSpeedTests)); }
  catch (e) { /* Measurement still works if browser storage is unavailable. */ }
}
function srLoadNodes() {
  srApi('node_details', null, function (d) {
    if (d.ok === false || !Array.isArray(d.nodes)) { srShowMsg(d.msg || '节点加载失败，请重试。','err'); return; }
    var previous = Object.create(null);
    (savedSpeedTests.results || []).forEach(function (entry) { previous[entry.key] = entry.latency; });
    SR_STATE.nodes.forEach(function (n,i) { if (SR_STATE.lat[i] != null) previous[nodeViewKey(n)] = SR_STATE.lat[i]; });
    SR_STATE.nodes = d.nodes; SR_STATE.lat = {};
    SR_STATE.nodes.forEach(function(n,i) { var key = nodeViewKey(n); if (previous[key] != null) SR_STATE.lat[i] = previous[key]; });
    if (!speedTestsRestored) {
      speedTestsRestored = true;
      // The backend owns running work and cached results, including after reload.
      pollSpeedTestJob();
    }
    SR_STATE.activeNode = d.active || 0;
    renderNodes();
    renderChrome();
  }, function () { SR_PAGE_LOADED['nodes'] = false; srElement('node-list').textContent = '加载失败，请点击刷新或重新打开此页。'; });
}
function renderNodes() {
  var el = srElement('node-list');
  srElement('nodes-count').textContent = SR_STATE.nodes.length + ' 个节点';
  if (!SR_STATE.nodes.length) { el.innerHTML = '<div class="hint">还没有节点，请添加订阅或手动添加节点。</div>'; return; }
  var html = '';
  var activeIdx = Number(SR_STATE.status.active_node);
  var query = srElement('node-search').value.toLowerCase().trim();
  SR_STATE.nodes.forEach(function (n, i) {
    if (query && (n.name + ' ' + n.protocol).toLowerCase().indexOf(query) < 0) return;
    var lat = SR_STATE.lat[i];
    html += '<div class="row' + (i === activeIdx ? ' active' : '') + '">' +
      '<span class="badge">' + srEsc(n.protocol || '?') + '</span>' +
      '<button class="r-name node-inspect" onclick="inspectNode(' + i + ')">' + srEsc(n.name) + '</button>' +
      '<span class="r-sub">' + (i === activeIdx ? '使用中' : '') + '</span>' +
      '<span class="lat ' + (lat != null ? latClass(lat) : '') + '" id="sr-lat-' + i + '">' + (lat != null ? latText(lat) : '—') + '</span>' +
      '<button class="btn btn-sm" onclick="inspectNode(' + i + ')">详情</button>' +
      '<button class="btn btn-sm" onclick="event.stopPropagation();testNode(' + i + ')">测速</button>' +
      '<button class="btn btn-accent btn-sm"' + (i === activeIdx || nodeSwitchPending ? ' disabled' : '') + ' onclick="setNode(' + i + ')">' + (i === activeIdx ? '使用中' : '使用') + '</button>' +
      '<button class="btn btn-danger btn-sm" onclick="event.stopPropagation();delNode(' + i + ')">删除</button>' +
      '</div>';
  });
  el.innerHTML = html || '<div class="hint">没有匹配的节点，请更换搜索条件。</div>';
}
var nodeSwitchPending = false, inspectedNodeKey = null;
function nodeViewKey(n) { return n.test_id || JSON.stringify([n.name, n.protocol, n.server, n.port]); }
function inspectNode(i) {
  var n = SR_STATE.nodes[i]; if (!n) return;
  inspectedNodeKey = nodeViewKey(n);
  srElement('detail-name').textContent = n.name;
  srElement('detail-protocol').textContent = n.protocol || '未知';
  srElement('detail-endpoint').textContent = '已隐藏';
  srElement('detail-reveal').checked = false;
  srElement('detail-latency').textContent = SR_STATE.lat[i] == null ? '未测速' : latText(SR_STATE.lat[i]);
  srElement('detail-connect').disabled = i === Number(SR_STATE.status.active_node);
  srElement('node-dialog').showModal();
}
function inspectedNodeIndex() { for (var i=0;i<SR_STATE.nodes.length;i++) if(nodeViewKey(SR_STATE.nodes[i])===inspectedNodeKey) return i; return -1; }
function revealNodeEndpoint() {
  var i = inspectedNodeIndex(), el = srElement('detail-endpoint');
  el.textContent = i >= 0 && srElement('detail-reveal').checked ? SR_STATE.nodes[i].server + ':' + SR_STATE.nodes[i].port : '已隐藏';
}
function useInspectedNode() {
  var i = inspectedNodeIndex();
  if (i < 0) { srShowMsg('节点列表已变化，请重新打开详情。', 'err'); return; }
  srElement('node-dialog').close(); setNode(i);
}
function setNode(i) {
  if (nodeSwitchPending || !SR_STATE.nodes[i]) return;
  nodeSwitchPending = true; renderNodes();
  function complete() { nodeSwitchPending = false; renderNodes(); }
  srApi('set_node', { index: i }, function (d) {
    complete(); toastResp(d);
    if (d.ok === false) return;
    srLoadNodes(); refreshStatus(true);
    // Selection and connectivity testing are separate user actions.
  }, complete);
}
function testNode(i) {
  if (!SR_STATE.nodes[i]) return;
  var key = nodeViewKey(SR_STATE.nodes[i]);
  var el = srElement('lat-' + i);
  if (el) el.textContent = '…';
  // Warm through-proxy probe (warmup + measure). Cap wait for Reality/QUIC.
  srApi('speedtest_node', { index: i }, function (d) {
    if (!SR_STATE.nodes[i] || nodeViewKey(SR_STATE.nodes[i]) !== key) return;
    SR_STATE.lat[i] = d.ok && d.latency >= 0 ? d.latency : -1;
    saveSpeedTests();
    renderNodes();
    renderChrome();
  }, function () {
    if (!SR_STATE.nodes[i] || nodeViewKey(SR_STATE.nodes[i]) !== key) return;
    SR_STATE.lat[i] = -1;
    saveSpeedTests();
    renderNodes();
    renderChrome();
  }, 60000);
}
var speedTestPollPending = false;
var speedTestPollTimer = null;
function applySpeedTestJob(job) {
  var indices = Object.create(null);
  SR_STATE.nodes.forEach(function (node, i) { indices[nodeViewKey(node)] = i; });
  (job.results || []).forEach(function (result) {
    var index = indices[result.key];
    if (index != null) SR_STATE.lat[index] = result.latency;
  });
  SR_STATE.testingAll = job.state === 'running';
  var btn = srElement('test-all-btn');
  btn.disabled = SR_STATE.testingAll;
  btn.textContent = SR_STATE.testingAll ? '正在测速 ' + job.completed + '/' + job.total : '快速测速';
  // Once per poll, rather than rebuilding and saving the list for every node.
  saveSpeedTests();
  renderNodes();
  renderChrome();
  if (job.state === 'interrupted') srShowMsg('后台测速已中断，已完成的结果已保留，请重新测速。', 'err');
}
function pollSpeedTestJob() {
  if (speedTestPollPending) return;
  if (speedTestPollTimer) { clearTimeout(speedTestPollTimer); speedTestPollTimer = null; }
  speedTestPollPending = true;
  srApi('speedtest_job', {}, function (data) {
    speedTestPollPending = false;
    if (!data.ok || !data.job) return;
    applySpeedTestJob(data.job);
    if (data.job.state === 'running') speedTestPollTimer = setTimeout(pollSpeedTestJob, 1000);
  }, function () {
    speedTestPollPending = false;
    speedTestPollTimer = setTimeout(pollSpeedTestJob, 2000);
  });
}
function testAllNodes(mode) {
  if (SR_STATE.testingAll || !SR_STATE.nodes.length) return;
  SR_STATE.testingAll = true;
  var btn = srElement('test-all-btn');
  btn.disabled = true; btn.textContent = '正在启动测速…';
  var bytes = new Uint8Array(16);
  window.crypto.getRandomValues(bytes);
  var requestId = Array.prototype.map.call(bytes, function (b) { return ('0' + b.toString(16)).slice(-2); }).join('');
  srApi('speedtest_start', {mode: mode || 'tcp', request_id: requestId}, function (data) {
    if (!data.ok || !data.job) {
      SR_STATE.testingAll = false; btn.disabled = false; btn.textContent = '快速测速';
      srShowMsg(data.msg || '无法启动测速', 'err'); return;
    }
    applySpeedTestJob(data.job);
    pollSpeedTestJob();
  }, function () {
    // The start response may be lost after the worker launched. Reattach first.
    pollSpeedTestJob();
  });
}
function clearAddNodeSecrets() {
  ['an-pass','an-uuid','an-wg-private','an-wg-public','an-wg-psk'].forEach(function(id) { srElement(id).value = ''; });
}
function toggleAddNodeForm() {
  var card=srElement('add-node-card');card.classList.toggle('open');
  if (!card.classList.contains('open')) clearAddNodeSecrets();
  updateAddNodeFields();
}
function updateAddNodeFields() {
  var proto=srElement('an-proto').value;
  srElement('an-tls-fields').hidden=proto!=='anytls';
  srElement('an-wg-fields').hidden=proto!=='wireguard';
  srElement('an-pass').parentNode.hidden=['shadowsocks','trojan','hysteria2','tuic','anytls'].indexOf(proto)<0;
  srElement('an-uuid').parentNode.hidden=['vmess','vless','tuic'].indexOf(proto)<0;
  srElement('an-cipher').parentNode.hidden=['shadowsocks','vmess'].indexOf(proto)<0;
}
function addNode() {
  var data = {
    name: srElement('an-name').value.trim(),
    protocol: srElement('an-proto').value,
    server: srElement('an-server').value.trim(),
    port: Number(srElement('an-port').value),
    password: srElement('an-pass').value,
    uuid: srElement('an-uuid').value.trim(),
    cipher: srElement('an-cipher').value.trim(),
    sni: srElement('an-sni').value.trim(),
    skip_cert_verify: srElement('an-insecure').checked,
  };
  var split=function(value){return value.split(/[\s,]+/).filter(Boolean);};
  if(data.protocol==='wireguard') data.wireguard={private_key:srElement('an-wg-private').value.trim(),public_key:srElement('an-wg-public').value.trim(),preshared_key:srElement('an-wg-psk').value.trim()||null,local_addresses:split(srElement('an-wg-address').value),allowed_ips:split(srElement('an-wg-allowed').value),dns:split(srElement('an-wg-dns').value),mtu:Number(srElement('an-wg-mtu').value),persistent_keepalive:Number(srElement('an-wg-keepalive').value),udp:true};
  var secrets={};['an-pass','an-uuid','an-wg-private','an-wg-public','an-wg-psk'].forEach(function(id){secrets[id]=srElement(id).value;});
  if (!data.name || !data.server || !Number.isInteger(data.port) || data.port < 1 || data.port > 65535) { srShowMsg('请填写名称、服务器和有效端口', 'err'); return; }
  srApi('add_node', data, function (d) {
    toastResp(d);
    if (d.ok !== false) { Object.keys(secrets).forEach(function(id){if(srElement(id).value===secrets[id])srElement(id).value='';}); srLoadNodes(); refreshStatus(); }
  });
}
function delNode(i) {
  if (!confirm('删除节点 ' + (SR_STATE.nodes[i] ? SR_STATE.nodes[i].name : '#' + i) + ' ?')) return;
  srApi('del_node', { index: i }, function (d) {
    toastResp(d);
    if (d.ok !== false) { srLoadNodes(); refreshStatus(); }
  });
}

/* ══ Subscriptions ═════════════════════════════════════════════════ */
function srLoadSubs() {
  restoreSubscriptionJob();
  srApi('subscriptions', null, function (d) {
    var el = srElement('sub-list');
    if (d.ok === false || !Array.isArray(d.subs)) { srShowMsg(d.msg || '订阅加载失败，请重试。','err'); return; }
    var subs = d.subs;
    SR_STATE.subs = subs;
    if (!subs.length) { el.innerHTML = '<div class="hint">还没有订阅，请先添加订阅。</div>'; return; }
    var shared = Number(d.shared_nodes || 0);
    var sharedHint = shared > 0 ? '<div class="hint">其中 ' + shared + ' 个节点由多个订阅共用，节点列表中各只显示一次。</div>' : '';
    el.innerHTML = sharedHint + subs.map(function (s) {
      return '<div class="row">' +
        '<span class="badge acc">' + srEsc(s.format || 'auto') + '</span>' +
        '<span class="r-name">' + srEsc(s.name) + '<span class="r-sub">已导入 ' + Number(s.node_count || 0) + ' 个节点 · 订阅地址已隐藏</span></span>' +
        '<button class="btn btn-sm" onclick="inspectSubscription(' + Number(s.index) + ')">详情</button>' +
        '<button class="btn btn-sm" onclick="event.stopPropagation();renameSub(' + Number(s.index) + ')" title="重命名订阅">重命名</button>' +
        '<button class="btn btn-sm" onclick="event.stopPropagation();copySubUrl(' + Number(s.index) + ')" title="复制私有订阅地址">复制地址</button>' +
        '<button class="btn btn-danger btn-sm" onclick="event.stopPropagation();delSub(' + Number(s.index) + ')">删除</button>' +
        '</div>';
    }).join('');
  }, function () { SR_PAGE_LOADED['subs'] = false; srElement('sub-list').textContent = '加载失败，请点击刷新或重新打开此页。'; });
}
function findSubscription(i) {
  return SR_STATE.subs.filter(function (sub) { return Number(sub.index) === Number(i); })[0];
}
var inspectedSubIndex = null, inspectedSubUrl = null;
function inspectSubscription(i) {
  var sub = findSubscription(i); if (!sub) return;
  inspectedSubIndex = i; inspectedSubUrl = sub.url;
  srElement('sub-detail-name').textContent = sub.name;
  srElement('sub-detail-format').textContent = sub.format || 'auto';
  srElement('sub-detail-url').textContent = '已隐藏';
  srElement('sub-detail-reveal').checked = false;
  srElement('sub-dialog').showModal();
}
function revealSubscriptionUrl() {
  var sub = findSubscription(inspectedSubIndex);
  srElement('sub-detail-url').textContent = sub && sub.url === inspectedSubUrl && srElement('sub-detail-reveal').checked ? sub.url : '已隐藏';
}
function renameSub(i) {
  var subs = SR_STATE.subs || [];
  var s = null;
  for (var k = 0; k < subs.length; k++) {
    if (subs[k].index === i) { s = subs[k]; break; }
  }
  if (!s) s = subs[i];
  if (!s) { srShowMsg('未找到该订阅', 'err'); return; }
  var next = prompt('重命名订阅', s.name || '');
  if (next === null) return;
  next = String(next).trim();
  if (!next) { srShowMsg('名称不能为空', 'err'); return; }
  srApi('rename_sub', { index: i, name: next }, function (d) {
    toastResp(d);
    if (d.ok !== false) srLoadSubs();
  });
}
function copySubUrl(i) {
  var subs = SR_STATE.subs || [];
  var s = null;
  for (var k = 0; k < subs.length; k++) {
    if (subs[k].index === i) { s = subs[k]; break; }
  }
  if (!s) s = subs[i];
  if (!s || !s.url) { srShowMsg('订阅地址不存在', 'err'); return; }
  srCopyText(s.url, '已复制订阅地址');
}
var subscriptionPending = false, subscriptionJobId = null, subscriptionJobTimer = null;
var subscriptionJobChecked = false, subscriptionJobSnapshot = null;
var subscriptionAwaitingLaunch = false, subscriptionLaunchError = null, subscriptionMissingPolls = 0;
var subscriptionDiagnostic = '';
var subscriptionNodesLoadedJobId = null;
var subscriptionJobStartedAt = 0;
function subscriptionBusy(on) { subscriptionPending = on; srElement('sub-add').disabled = on; srElement('sub-update').disabled = on; }
function renderSubscriptionNotes(result) {
  var notes = result && result.compatibility_notes;
  var hint = Array.isArray(notes) && notes.indexOf('hysteria2_browser_fingerprint_hint') >= 0;
  // Older saved jobs embedded the metadata-only note in their success message.
  if (!hint && result && result.ok !== false) hint = String(result.msg || '').indexOf('Hysteria2 uses native QUIC TLS; browser fingerprint hints were retained as metadata') >= 0;
  var chrome = Array.isArray(notes) && notes.indexOf('hysteria2_chrome_quic_profile') >= 0;
  var fixedPort = Array.isArray(notes) && notes.indexOf('hysteria2_mport_fixed_port') >= 0;
  var text = [];
  if (fixedPort) text.push('部分 Hysteria2 节点带有多端口跳跃配置，已按订阅地址中的固定端口导入；当前版本不会定期切换端口。若网络限制持续使用同一 UDP 端口，这些节点仍可能无法连接。');
  if (chrome) text.push('已启用 Chrome 115 基线 QUIC 握手模拟（fp=chrome / chrome115，ALPN h3），包含 TLS 指纹、连接 ID 和传输参数。此基线不模拟 ALPS、浏览器 HTTP/3 请求及首包分帧；证书校验保持原样。');
  if (hint) text.push('其他浏览器指纹提示已保留，但不用于模拟对应浏览器的 QUIC 指纹；证书校验保持原样。');
  srElement('sub-notes-text').textContent = text.join(' ');
  srElement('sub-notes').hidden = !text.length;
}
function scheduleSubscriptionPoll(delay) {
  clearTimeout(subscriptionJobTimer);
  if (typeof delay !== 'number') delay = Date.now() - subscriptionJobStartedAt < 5000 ? 500 : 2000;
  subscriptionJobTimer = setTimeout(function () { pollSubscriptionJob(subscriptionJobId); }, delay);
}
function normalizeSubscriptionError(d) {
  // Jobs saved by older packages only contain msg. Recognize exact safe
  // reasons so refreshing the UI can explain those historical failures too.
  if (d && (d.error_code === 'subscription_exists' || d.msg === 'This subscription already exists; update the existing entry')) {
    return {ok:false,error_code:'subscription_exists',stage:'validate_subscription',cause:'DuplicateSubscription',msg:'该订阅已存在，请更新已有订阅。',hint:'在“我的订阅”中点击“全部更新”，无需重复添加。'};
  }
  if (d && !d.error_code && /No valid proxy nodes found in subscription|Subscription contains no usable nodes/.test(String(d.msg || ''))) {
    return {ok:false,error_code:'subscription_no_valid_nodes',stage:'parse_subscription',cause:'NoValidNodes',msg:'订阅已下载，但没有解析出可用节点。',hint:'核对订阅格式及节点参数兼容性；现有节点会保留。'};
  }
  return d;
}
function subscriptionErrorText(d, fallback) {
  d = normalizeSubscriptionError(d);
  var text = d && (d.msg || d.error_message || d.error || d.error_code) || fallback;
  if (d && d.stage) text += ' [' + d.stage + (d.cause ? ': ' + d.cause : '') + ']';
  if (d && d.error_code) text += '\n错误码：' + d.error_code;
  if (d && typeof d.os_error === 'number') text += '\n系统错误码：' + d.os_error;
  if (d && typeof d.exit_code === 'number') text += '\n进程退出码：' + d.exit_code;
  if (d && d.hint) text += '\n建议：' + d.hint;
  return text;
}
function showSubscriptionDiagnostic(d, jobId) {
  d = normalizeSubscriptionError(d) || {};
  var stages = {read_request:'读取接口请求',parse_request:'解析接口请求',invoke_backend:'调用后端程序',prepare_directory:'准备数据目录',lock_job:'获取任务锁',save_request:'保存订阅请求',save_job:'保存任务状态',resolve_worker:'定位后台程序',spawn_worker:'启动后台进程',fetching:'下载订阅',applying:'保存并应用配置'};
  var causes = {InvalidBase64:'请求编码无效',InvalidUtf8:'请求不是有效 UTF-8 文本',InvalidJson:'请求 JSON 无效或被软件中心转义',UnexpectedEof:'请求内容被截断',TooLarge:'请求内容超过大小限制',PermissionDenied:'权限不足',AlreadyExists:'路径已被其他文件占用',NotADirectory:'数据路径不是目录',IsADirectory:'文件路径实际为目录',NotFound:'程序或路径缺失',StorageFull:'存储空间不足',ReadOnlyFilesystem:'文件系统只读',WouldBlock:'资源暂被占用',Unsupported:'操作或文件系统能力不支持',Other:'系统调用失败'};
  var codes = ['request_body_invalid','request_body_too_large','request_body_read_failed','backend_failed','backend_unavailable','backend_empty_response','subscription_start_failed','subscription_job_not_found','subscription_worker_exited','timeout','network_error','http_error','parse_error','busy','dispatch_error'];
  stages.validate_subscription = '校验订阅'; stages.parse_subscription = '解析订阅节点';
  causes.DuplicateSubscription = '该订阅地址已存在'; causes.NoValidNodes = '返回内容没有可用节点或包含不兼容参数'; causes.InvalidUrl = '订阅地址格式无效';
  codes.push('subscription_exists','subscription_no_valid_nodes','subscription_url_invalid','subscription_update_failed');
  var code = codes.indexOf(d.error_code) >= 0 ? d.error_code : 'subscription_failed';
  var reason = causes[d.cause] || (code === 'timeout' ? '请求超时' : code === 'network_error' ? '无法连接插件接口' : code === 'busy' ? '接口正在处理其他请求' : '订阅任务未完成，请结合阶段与日志排查');
  var hint = d.cause === 'PermissionDenied' ? '检查插件数据目录及程序权限，保留已有配置。' : d.cause === 'StorageFull' || d.os_error === 28 || d.os_error === 122 ? '检查并释放路由器存储空间。' : d.cause === 'ReadOnlyFilesystem' || d.os_error === 30 ? '检查存储挂载及文件系统错误。' : '请反馈错误码、失败阶段和对应时间的脱敏日志。';
  if (d.stage === 'parse_request' || d.stage === 'read_request' || d.stage === 'invoke_backend') hint = d.cause === 'TooLarge' ? '缩小单次提交内容后重试。' : '通过软件中心安装完整插件包后刷新页面；反馈错误码与进程退出码。';
  if (code === 'subscription_exists') hint = '在“我的订阅”中点击“全部更新”，无需重复添加。';
  if (code === 'subscription_no_valid_nodes') hint = '核对订阅格式及节点参数兼容性；现有节点会保留。';
  if (code === 'subscription_url_invalid') hint = '填写完整的 http 或 https 订阅地址；名称和格式可以留空。';
  if (code === 'subscription_update_failed') { reason = '所有订阅均未更新，原有节点已保留'; hint = '核对错误地址、服务端状态及节点参数兼容性后重试。'; }
  var status = /\bHTTP(?:\/\d(?:\.\d)?)?\s+([1-5]\d{2})\b/.exec(String(d.msg || d.error || ''));
  if (status) {
    reason = '订阅服务器返回 HTTP ' + status[1];
    hint = /^(401|403)$/.test(status[1]) ? '核对订阅权限、有效期及服务端访问限制。' : status[1] === '429' ? '服务端限制请求频率，请稍后重试。' : '核对订阅服务器状态及链接有效性。';
  }
  // Only allowlisted metadata enters the copyable report. Raw messages may
  // contain server content or personal paths and are deliberately excluded.
  var lines = ['错误码：' + code,'失败阶段：' + (stages[d.stage] || '未确认'),'原因：' + reason];
  if (typeof d.os_error === 'number' && isFinite(d.os_error)) lines.push('系统错误码：' + d.os_error);
  if (typeof d.exit_code === 'number' && isFinite(d.exit_code)) lines.push('进程退出码：' + d.exit_code);
  if (/^[0-9a-f]{32}$/.test(jobId || '')) lines.push('任务 ID：' + jobId);
  lines.push('建议：' + hint);
  subscriptionDiagnostic = lines.join('\n');
  srElement('sub-diagnostic-text').textContent = subscriptionDiagnostic;
  srElement('sub-diagnostic').hidden = false;
}
function copySubscriptionDiagnostic() { srCopyText(subscriptionDiagnostic, '已复制脱敏诊断'); }
function failSubscriptionJob(d, fallback) {
  showSubscriptionDiagnostic(d, subscriptionJobId);
  clearTimeout(subscriptionJobTimer);
  subscriptionBusy(false); subscriptionJobId = null; subscriptionJobSnapshot = null;
  subscriptionAwaitingLaunch = false;
  srShowMsg(subscriptionErrorText(d, fallback), 'err');
}
function subscriptionJobMissing() {
  if (subscriptionAwaitingLaunch && ++subscriptionMissingPolls < 4) {
    srShowMsg('启动结果尚未确认，正在核对同一任务；输入内容已保留。', 'info');
    scheduleSubscriptionPoll(2000); return;
  }
  failSubscriptionJob(subscriptionLaunchError, '未能确认订阅任务，请刷新列表后重试；输入内容已保留。');
}
function acceptSubscriptionJob(d) {
  if (d && d.ok === false) {
    if (d.error_code === 'subscription_job_not_found') subscriptionJobMissing();
    else failSubscriptionJob(d, '订阅任务失败；输入内容已保留。');
    return;
  }
  var job = d && d.job;
  if (!job || !job.id || (job.state !== 'running' && job.state !== 'finished')) {
    subscriptionJobMissing(); return;
  }
  subscriptionAwaitingLaunch = false; subscriptionLaunchError = null; subscriptionMissingPolls = 0;
  subscriptionJobChecked = true;
  subscriptionJobId = job.id;
  if (job.state === 'running') {
    if (!subscriptionJobStartedAt) subscriptionJobStartedAt = Date.now();
    subscriptionBusy(true);
    var counts = job.total ? '（' + Number(job.completed || 0) + '/' + Number(job.total) + '）' : '';
    var saved = job.nodes_saved === true;
    var message = job.stage === 'applying' ? (saved ? '节点已保存，正在应用设置' : '正在保存并应用订阅') : job.stage === 'saving' ? '正在保存订阅' : '正在后台下载订阅';
    if (job.stage === 'applying' && saved && subscriptionNodesLoadedJobId !== job.id) {
      subscriptionNodesLoadedJobId = job.id;
      srLoadSubs(); srLoadNodes();
    }
    srShowMsg(message + counts + '，关闭页面后仍会继续。', 'info');
    scheduleSubscriptionPoll(); return;
  }
  subscriptionBusy(false); subscriptionJobId = null;
  clearTimeout(subscriptionJobTimer);
  var result = job.result || {ok:false,msg:'订阅任务未返回结果，请刷新列表确认。'};
  if (result.ok !== false && subscriptionJobSnapshot) {
    var snapshot = subscriptionJobSnapshot;
    if (srElement('sub-name').value.trim() === snapshot.name && srElement('sub-url').value.trim() === snapshot.url) {
      srElement('sub-name').value = ''; srElement('sub-url').value = '';
    }
  }
  subscriptionJobSnapshot = null;
  renderSubscriptionNotes(result);
  if (result.ok === false) { showSubscriptionDiagnostic(result, job.id); srShowMsg(subscriptionErrorText(result, '订阅任务失败'), 'err'); }
  else { srElement('sub-diagnostic').hidden = true; toastResp(result); }
  srLoadSubs(); srLoadNodes(); refreshStatus();
}
function pollSubscriptionJob(id) {
  srApi('subscription_job', id ? {id:id} : {}, acceptSubscriptionJob, function (error) {
    if (error && error.error_code === 'subscription_job_not_found') { subscriptionJobMissing(); return; }
    srShowMsg('暂时无法读取订阅进度，正在重试；刷新页面也可继续查看。', 'info');
    scheduleSubscriptionPoll(2000);
  }, 12000);
}
function restoreSubscriptionJob() {
  if (subscriptionJobChecked) return;
  subscriptionJobChecked = true;
  srApi('subscription_job', {}, function (d) {
    if (d.ok !== false && d.job && !subscriptionPending) {
      if (d.job.state === 'running') acceptSubscriptionJob(d);
      else if (d.job.result && d.job.result.ok === false) showSubscriptionDiagnostic(d.job.result, d.job.id);
      else if (d.job.result) renderSubscriptionNotes(d.job.result);
    }
  }, function () { subscriptionJobChecked = false; }, 12000);
}
function startSubscriptionJob(action, body) {
  subscriptionNodesLoadedJobId = null;
  subscriptionJobStartedAt = Date.now();
  var random = new Uint8Array(16);
  try { window.crypto.getRandomValues(random); }
  catch (error) { failSubscriptionJob(null, '浏览器无法生成安全任务 ID，请使用现代浏览器。'); return; }
  subscriptionJobId = Array.prototype.map.call(random, function (b) { return ('0' + b.toString(16)).slice(-2); }).join('');
  subscriptionAwaitingLaunch = true; subscriptionLaunchError = null; subscriptionMissingPolls = 0;
  subscriptionBusy(true);
  srShowMsg('正在启动后台订阅任务…', 'info');
  srApi('subscription_start', {request_id:subscriptionJobId,action:action,body:body}, function (d) {
    if (d && d.existing) subscriptionJobSnapshot = null;
    if (!d || (!d.job && d.ok !== false)) {
      subscriptionLaunchError = {msg:'后台未返回有效任务，可能是页面与后端版本不一致。'};
      pollSubscriptionJob(subscriptionJobId); return;
    }
    acceptSubscriptionJob(d);
  }, function (error) {
    if (error && error.error_code === 'subscription_start_failed') {
      failSubscriptionJob(error, '订阅后台启动失败；输入内容已保留。'); return;
    }
    subscriptionLaunchError = error || {msg:'订阅启动请求未收到响应。'};
    srShowMsg('正在确认后台订阅任务状态…', 'info');
    pollSubscriptionJob(subscriptionJobId);
  }, 12000);
}
function addSub() {
  if (subscriptionPending) return;
  var data = {name:srElement('sub-name').value.trim(),url:srElement('sub-url').value.trim(),format:srElement('sub-format').value};
  if (!data.url) { srShowMsg('请填写订阅地址（必填）。', 'err'); return; }
  if (!/^https?:\/\//i.test(data.url)) { srShowMsg('请输入 http 或 https 订阅地址', 'err'); return; }
  try {
    var parsed = new URL(data.url);
    if (!parsed.hostname || (parsed.protocol !== 'http:' && parsed.protocol !== 'https:')) throw new Error('invalid URL');
  } catch (error) { srShowMsg('订阅地址无效，请填写完整的 http 或 https 地址。', 'err'); return; }
  subscriptionJobSnapshot = data;
  startSubscriptionJob('add_sub', data);
}
function delSub(i) {
  if (!confirm('删除订阅 #' + i + ' ?')) return;
  srApi('del_sub', { index: i }, function (d) {
    toastResp(d);
    if (d.ok !== false) { srLoadSubs(); refreshStatus(); }
  });
}
function updateSubs() {
  if (subscriptionPending) return;
  subscriptionJobSnapshot = null;
  startSubscriptionJob('update_subs', {});
}


var SR_PROXY_GROUPS=[], SR_RULE_NODES=[], PG_NODES=[], PG_REVISION=null, PG_EDIT_REVISION=null, PG_ORIGINAL='', PG_KEEP_CURRENT='', PG_BUSY=false, PG_LOAD_GENERATION=0;
function setProxyGroupTargets(groups) {
  SR_PROXY_GROUPS=groups||[];
  if (typeof rebuildRuleTargets==='function') { rebuildRuleTargets(); return; }
  Object.keys(RULE_TARGET_LABEL).forEach(function(k){if(k.indexOf('group:')===0)delete RULE_TARGET_LABEL[k];});
  SR_PROXY_GROUPS.forEach(function(g){RULE_TARGET_LABEL['group:'+g.name]='分组 · '+g.name;});
  ['rule-target','edit-rule-target','rule-filter-target','device-target','device-add-target','preset-route-target'].forEach(function(id){
    var el=srElement(id);if(!el)return;var old=el.value;
    var prefix=id==='device-add-target'?'<option value="">请选择出口（必填）</option>':id==='rule-filter-target'?'<option value="">全部处理方式</option>':id==='device-target'?'<option value="inherit">清除自定义默认出口</option>':id==='preset-route-target'?'<option value="">使用预设默认</option>':'';
    el.innerHTML=prefix+['proxy','direct','reject'].concat(SR_PROXY_GROUPS.map(function(g){return 'group:'+g.name;})).map(function(t){return '<option value="'+srEsc(t)+'">'+srEsc(RULE_TARGET_LABEL[t])+'</option>';}).join('');
    if(Array.from(el.options).some(function(o){return o.value===old;}))el.value=old;
    else if(old && old.indexOf('group:')===0){var o=document.createElement('option');o.value=old;o.textContent='已失效 · '+old;el.appendChild(o);el.value=old;}
  });
}
function loadProxyGroups(){
  if(PG_BUSY)return;
  var generation=++PG_LOAD_GENERATION;
  srApi('proxy_groups',null,function(d){
    if(generation!==PG_LOAD_GENERATION)return;
    if(!d.ok||!Array.isArray(d.groups)||!Array.isArray(d.nodes)){toastResp(d);return;}
    PG_NODES=d.nodes;PG_REVISION=d.revision;setProxyGroupTargets(d.groups);renderProxyGroups();
  },function(){srShowMsg('节点分组读取失败，请刷新重试。','err');});
}
function renderProxyGroups(){
  srElement('proxy-group-list').innerHTML=SR_PROXY_GROUPS.length?SR_PROXY_GROUPS.map(function(g,i){return '<div class="toolbar" style="margin-bottom:10px"><strong>'+srEsc(g.name)+'</strong><span class="hint">'+g.members.length+' 个成员 · '+srEsc(g.selected_node||'当前成员不可用')+(g.missing_members?' · '+g.missing_members+' 个失效成员':'')+'</span><div class="spacer"></div><button class="btn btn-sm" onclick="editProxyGroup('+i+')">查看 / 编辑</button><button class="btn btn-sm" onclick="deleteProxyGroup('+i+')">删除</button></div>';}).join(''):'<p class="hint">尚无分组。新建后可在规则、策略和设备出口中选择。</p>';
}
function editProxyGroup(i){
  if(PG_BUSY||!PG_REVISION){srShowMsg('请先刷新分组。','err');return;}
  var g=SR_PROXY_GROUPS[i]||{name:'',type:'select',members:[],current:null};
  PG_ORIGINAL=g.name;PG_EDIT_REVISION=PG_REVISION;
  srElement('pg-name').value=g.name;srElement('pg-type').value=g.type;
  var ns=g.members.map(function(fp){return PG_NODES.find(function(n){return n.fingerprint===fp;})||{fingerprint:fp,name:'失效成员（可移除）'};}).concat(PG_NODES.filter(function(n){return g.members.indexOf(n.fingerprint)<0;}));
  PG_KEEP_CURRENT=g.current||'';
  var rows=ns.map(function(n){return {value:n.fingerprint,text:n.name,selected:g.members.indexOf(n.fingerprint)>=0};});
  writeProxyGroupMembers(rows);
  updateProxyGroupMode();
  srElement('proxy-group-editor').hidden=false;
}
function proxyGroupMemberState(){
  var host=srElement('pg-members');
  if(host&&typeof host.querySelectorAll==='function'){
    return Array.from(host.querySelectorAll('.pg-member')).map(function(row){
      var input=row.querySelector('input');
      var span=row.querySelector('span');
      return {value:input.value,selected:!!input.checked,text:span?span.textContent:''};
    });
  }
  return (host.options||[]).map(function(option){return {value:option.value,selected:!!option.selected,text:option.textContent||''};});
}
function renderProxyGroupCurrent(){
  var chosen=proxyGroupMemberState().filter(function(row){return row.selected;});
  if(!chosen.some(function(row){return row.value===PG_KEEP_CURRENT;}))PG_KEEP_CURRENT=chosen.length?chosen[0].value:'';
  var picker=srElement('pg-current');
  picker.innerHTML=chosen.length?chosen.map(function(row){return deviceOption(row.value,row.text);}).join(''):deviceOption('','请先勾选组内成员');
  picker.value=PG_KEEP_CURRENT;picker.disabled=!chosen.length;
  srElement('pg-current-field').hidden=srElement('pg-type').value!=='select';
}
function writeProxyGroupMembers(rows){
  var type=srElement('pg-type').value,manual=type==='select',ordered=type==='fallback';
  srElement('pg-members').innerHTML=rows.map(function(row){
    var input='<input type="checkbox" value="'+srEsc(row.value)+'"'+(row.selected?' checked':'')+' onchange="setProxyGroupMember(this.value,this.checked)">';
    var move=ordered?'<span class="pg-member-move"><button type="button" class="btn btn-sm" onclick="moveProxyGroupMember(this,-1)">上移</button><button type="button" class="btn btn-sm" onclick="moveProxyGroupMember(this,1)">下移</button></span>':'';
    return '<div class="pg-member"><label>'+input+'<span>'+srEsc(row.text)+'</span></label>'+move+'</div>';
  }).join('');
  renderProxyGroupCurrent();
}
function pickProxyGroupMember(value){PG_KEEP_CURRENT=value;}
function setProxyGroupMember(value,selected){
  var rows=proxyGroupMemberState();
  rows.forEach(function(row){if(row.value===value)row.selected=!!selected;});
  writeProxyGroupMembers(rows);
}
function setProxyGroupMembers(selected){
  var rows=proxyGroupMemberState();
  rows.forEach(function(row){row.selected=!!selected;});
  writeProxyGroupMembers(rows);
}
function updateProxyGroupMode(){
  var type=srElement('pg-type').value,manual=type==='select',ordered=type==='fallback';
  var rows=proxyGroupMemberState();
  writeProxyGroupMembers(rows);
  var label=srElement('pg-members-label');
  if(label)label.textContent=manual?'组内成员':(ordered?'成员（上面的先用）':'参与测速的节点');
  var hint=srElement('pg-member-hint');
  if(hint)hint.textContent=manual
    ?'可勾选多个成员，再在下方选择当前使用节点。取消当前节点的勾选后，将默认选中剩余的第一个成员，保存后生效。'
    :ordered
    ?'可勾选多个。上面的先用，失败再试下一个。上次成功的会暂时沿用，并回头尝试更靠前的；UDP 固定走第一个。'
    :'可勾选多个。这些节点一起比延迟，五分钟内沿用更快的那个。顺序不决定用哪一个。';
  var tools=srElement('pg-member-tools');
  if(tools)tools.hidden=false;
}
function proxyGroupCurrent(){
  var chosen=proxyGroupMemberState().filter(function(row){return row.selected;});
  if(!chosen.length)return null;
  if(chosen.some(function(row){return row.value===PG_KEEP_CURRENT;}))return PG_KEEP_CURRENT;
  return chosen[0].value;
}
function moveProxyGroupMember(value,direction){
  if(value&&typeof value.closest==='function'){
    var row=value.closest('.pg-member');
    var input=row&&row.querySelector('input');
    value=input?input.value:'';
  }
  var rows=proxyGroupMemberState();
  var at=rows.findIndex(function(row){return row.value===value;});
  var to=at+direction;
  if(at<0||to<0||to>=rows.length)return;
  var swap=rows[at];rows[at]=rows[to];rows[to]=swap;
  writeProxyGroupMembers(rows);
}
function mutateProxyGroup(action,body){
  if(PG_BUSY)return;PG_BUSY=true;PG_LOAD_GENERATION++;srElement('pg-save').disabled=true;
  function done(d){PG_BUSY=false;srElement('pg-save').disabled=false;toastResp(d||{ok:false,msg:'请求未完成，请刷新核对保存状态'});if(d&&d.ok)srElement('proxy-group-editor').hidden=true;loadProxyGroups();SR_PAGE_LOADED.dns=false;invalidateRuleTrace('分组已更新，请刷新规则后重新检查。');}
  srApi(action,body,done,done,60000);
}
function saveProxyGroup(){
  mutateProxyGroup('save_proxy_group',{revision:PG_EDIT_REVISION,original_name:PG_ORIGINAL,group:{name:srElement('pg-name').value.trim(),type:srElement('pg-type').value,members:proxyGroupMemberState().filter(function(row){return row.selected;}).map(function(row){return row.value;}),current:proxyGroupCurrent()}});
}
function deleteProxyGroup(i){var g=SR_PROXY_GROUPS[i];if(!g||!confirm('删除分组 '+g.name+'？被规则引用时会拒绝删除。'))return;mutateProxyGroup('delete_proxy_group',{revision:PG_REVISION,name:g.name});}
function deviceOption(value, label) {
  return '<option value="'+srEsc(value)+'">'+srEsc(label)+'</option>';
}
function deviceSource() {
  return srElement('device-source').value.trim();
}
function deviceRecord() {
  return (SR_STATE.devices || []).find(function(row){return sameDeviceSource(row.source, deviceSource());});
}
function keepRulePickerPosition(picker, update) {
  var top = picker.getBoundingClientRect().top;
  update();
  // Updating the graph or the list above the picker can change its page offset.
  var shift = picker.getBoundingClientRect().top - top;
  if (Math.abs(shift) >= 1) window.scrollBy(0, shift);
}
function renderDevicePolicies() {
  srElement('device-form').disabled=ruleWorkspaceLocked();
  var picker=srElement('device-picker'), previous=picker.value, known=new Map(), networks=new Set();
  function addSource(source,label) {
    if(!source || known.has(source))return;
    var key=flowSourceKey(source);
    if(!key)return;
    if(networks.has(key))return;
    networks.add(key);
    known.set(source,label);
  }
  (SR_STATE.devices || []).forEach(function(d){addSource(d.source,d.alias || d.host || '设备');});
  (SR_STATE.ruleRows || []).concat(SR_STATE.deviceBindings || []).forEach(function(r){addSource(r.source,'已配置');});
  if(previous && previous!=='global' && previous!=='manual')addSource(previous,'已选择');
  picker.innerHTML=deviceOption('global','全局设备')+
    Array.from(known,function(entry){return deviceOption(entry[0],entry[1]+' · '+entry[0].replace(entry[0].indexOf(':')<0?/\/32$/:/\/128$/,''));}).join('')+
    deviceOption('manual','手动输入 IP / 网段');
  picker.value=previous==='manual'||previous==='global'?previous:
    (Array.from(known.keys()).find(function(source){return sameDeviceSource(source,previous);})||'global');
  selectPolicyDevice();
}
function selectPolicyDevice() {
  var value=srElement('device-picker').value, field=srElement('device-source');
  if(value!=='manual')field.value=value==='global'?'':value;
  else if(srElement('device-manual').hidden)field.value='';
  srElement('device-manual').hidden=value!=='manual';
  updateDevicePolicySelection();
}
// Drafts live only in this page and are keyed by the actual destination scope.
var deviceDrafts=new Map(), activeDraftScope=null, bindingBase=null;
function bindingFields() {
  return {priority:srElement('device-group-priority').value,target:srElement('device-group-target').value};
}
function changedBinding() {
  if(!bindingBase || srElement('device-bound-editor').hidden)return null;
  var fields=bindingFields();
  return fields.priority!==bindingBase.priority || fields.target!==bindingBase.target ? fields : null;
}
function switchDevicePanel(panel) {
  if(changedBinding() && !confirm('放弃这条绑定尚未保存的修改？'))return;
  srElement('device-bound-editor').hidden=true;
  [['device-add-groups','add'],['device-fallback','fallback'],['device-alias-panel','alias']].forEach(function(pair){srElement(pair[0]).hidden=panel!==pair[1];});
}
function cancelDevicePanel() {
  if(confirm('取消当前操作并放弃未保存的修改？')){
    deviceDrafts.delete(activeDraftScope);activeDraftScope=null;bindingBase=null;
    switchDevicePanel('');updateDevicePolicySelection();
  }
}
function closeDeviceBinding() {
  if(changedBinding() && !confirm('放弃这条绑定尚未保存的修改？'))return;
  srElement('device-bound-editor').hidden=true;
}
function deviceFieldDraft(id) {
  var field=srElement(id);
  if(field.value===field._savedValue || field._savedValue===undefined)return null;
  return {value:field.value,base:field._draftBase,key:field._deviceKey};
}
function restoreDeviceField(id,saved,draft,key) {
  var field=srElement(id);
  var pending=draft && draft.key===key && draft.value!==saved;
  field._savedValue=saved;
  field._deviceKey=key;
  field._draftBase=pending?draft.base:saved;
  field.value=pending?draft.value:saved;
}
function rememberDeviceDraft() {
  if(activeDraftScope===null)return;
  var edit=changedBinding(), groups=selectedDeviceGroups(), priority=srElement('device-add-priority').value;
  var fallback=deviceFieldDraft('device-target'), alias=deviceFieldDraft('device-alias'), addTarget=srElement('device-add-target').value;
  if(edit || groups.length || priority!=='0' || addTarget || fallback || alias){
    deviceDrafts.set(activeDraftScope,{edit:edit,group:srElement('device-group').value,base:bindingBase,groups:groups,priority:priority,addTarget:addTarget,fallback:fallback,alias:alias});
  }else deviceDrafts.delete(activeDraftScope);
}
function updateDevicePolicySelection() {
  var picker=srElement('device-picker').value,source=deviceSource(),global=picker==='global';
  srElement('device-scope-note').hidden=!global;
  var valid=global || (picker && !!sourceNetwork(source));
  rememberDeviceDraft();
  activeDraftScope=valid?(global?'global':flowSourceKey(source)):null;
  var draft=deviceDrafts.get(activeDraftScope);
  srElement('device-add-target').value=draft&&draft.addTarget||'';
  srElement('device-settings').hidden=!valid;
  srElement('device-specific').hidden=global;
  var editor=srElement('device-bound-editor');
  if(!valid || editor._source!==source){
    editor.hidden=true;
    srElement('device-fallback').open=false;
  }
  editor._source=source;
  if(!valid)return;
  var rows=orderedRules(SR_STATE.ruleRows||[]);
  var policy=rows.find(function(r){return isFinalRule(r) && r.enabled!==false && sameDeviceSource(r.source||'',source);});
  var exit=srElement('device-target');
  var current=policy?policy.target:'inherit';
  if(!Array.from(exit.options).some(function(o){return o.value===current;}))exit.innerHTML+=deviceOption(current,'已失效 · '+current);
  var pendingTarget=draft && draft.fallback;
  if(pendingTarget && !Array.from(exit.options).some(function(o){return o.value===pendingTarget.value;})){
    exit.innerHTML+=deviceOption(pendingTarget.value,'已失效 · '+pendingTarget.value);
  }
  restoreDeviceField('device-target',current,pendingTarget,activeDraftScope);
  var bindings=(SR_STATE.deviceBindings||[]).filter(function(b){return sameDeviceSource(source,b.source);}).sort(byBindingPriority);
  var inherited=(SR_STATE.globalBindings||[]).slice().sort(byBindingPriority);
  srElement('device-binding-title').textContent='使用的规则组'+(bindings.length?' · '+bindings.length+' 组':'');
  srElement('device-inherited').hidden=global || hasDeviceAssignment(source) || !inherited.length;
  srElement('device-inherited-title').textContent='当前使用全局设备规则 · '+inherited.length+' 组';
  srElement('device-inherited-list').innerHTML=inherited.map(function(b){
    return '<li>'+srEsc(b.group)+' · '+srEsc(b.target?(RULE_TARGET_LABEL[b.target]||b.target):'尚未指定出口（请编辑）')+'</li>';
  }).join('');
  var globalFinal=rows.find(function(r){return !r.source && isFinalRule(r) && r.enabled!==false;});
  var assigned=hasDeviceAssignment(source);
  var matchingFinal=source && rows.find(function(r){return r.source && isFinalRule(r) && r.enabled!==false && sourceContains(r.source,source);});
  var effectiveNote=srElement('device-effective-policy');
  effectiveNote.hidden=!matchingFinal || matchingFinal===policy;
  effectiveNote.textContent=effectiveNote.hidden?'':'当前按匹配顺序使用网段 '+matchingFinal.source+' 的出口。';
  var fallback=matchingFinal?matchingFinal.target:!assigned && globalFinal?globalFinal.target:'proxy';
  var action={proxy:'走代理',direct:'直连',reject:'拒绝连接'}[fallback] || '使用'+(RULE_TARGET_LABEL[fallback]||fallback);
  srElement('device-fallback-label').textContent='未命中规则时'+action;
  var automatic=Array.from(exit.options).find(function(o){return o.value==='inherit';});
  if(automatic)automatic.textContent=policy?'清除自定义默认出口':'未单独设置';
  srElement('device-bindings').innerHTML=bindings.map(function(binding){
    var target=binding.target?(RULE_TARGET_LABEL[binding.target]||binding.target):'尚未指定出口（请编辑）';
    return '<div class="row"><div class="r-name"><strong>'+srEsc(binding.group)+'</strong><span class="r-sub">优先级 '+Number(binding.priority||0)+' · '+srEsc(target)+'</span></div>'+
      '<button type="button" class="btn btn-sm" data-group="'+srEsc(binding.group)+'" onclick="editDeviceBinding(this.dataset.group)">编辑</button></div>';
  }).join('') || '<p class="hint">尚未关联规则组</p>';
  srElement('device-add-priority').value=draft?draft.priority:'0';
  var network=sourceNetwork(source);
  srElement('device-trace').hidden=!network || network.prefix!==network.width;
  var record=deviceRecord();
  restoreDeviceField('device-alias',record && record.alias || '',draft && draft.alias,(record && record.alias_key)||activeDraftScope);
  var bound=new Set(bindings.map(function(b){return b.group;})), available=new Set();
  rows.forEach(function(r){
    var name=(r.group||'').trim();
    if(!r.source && name && !isFinalRule(r) && !bound.has(name))available.add(name);
  });
  srElement('device-available-groups').innerHTML=Array.from(available,function(name){
    return '<label class="pg-member"><input type="checkbox" value="'+srEsc(name)+'"'+(draft && draft.groups.includes(name)?' checked':'')+'><span>'+srEsc(name)+'</span></label>';
  }).join('') || '<p class="hint">没有可添加的规则组</p>';
  if(editor._scope!==activeDraftScope){switchDevicePanel('');editor._scope=activeDraftScope;}
  updateDeviceAddCount();
  var group=srElement('device-group');
  if(!bound.has(group.value)){
    editor.hidden=true;
    group.value=bindings.length?bindings[0].group:'';
  }
  if(draft && draft.edit && bound.has(draft.group)){group.value=draft.group;editor.hidden=false;}
  if(bindings.length)renderDeviceBinding();
  if(draft && draft.edit && !editor.hidden && group.value===draft.group){
    if(draft.edit.priority!==bindingBase.priority || draft.edit.target!==bindingBase.target){
      srElement('device-group-priority').value=draft.edit.priority;
      srElement('device-group-target').value=draft.edit.target;
      if(draft.base.priority!==bindingBase.priority || draft.base.target!==bindingBase.target){
        srElement('device-group-state').textContent='这条绑定已被修改，请关闭编辑并重新核对。';
        srElement('device-link').disabled=true;
        bindingBase=draft.base;
      }
    }
  }
}
function selectedDeviceGroups() {
  return Array.from(srElement('device-available-groups').querySelectorAll('input:checked'),function(box){return box.value;});
}
function updateDeviceAddCount() {
  var count=selectedDeviceGroups().length, button=srElement('device-add-selected');
  button.disabled=!count;
  button.textContent=count?'关联所选 '+count+' 个组':'关联所选组';
}
function readBindingPriority(id) {
  var field=srElement(id), value=String(field.value).trim(), priority=Number(value);
  if(value && Number.isInteger(priority) && priority>=0 && priority<=65535)return priority;
  srShowMsg('关联优先级须为 0 至 65535 的整数。','err');
  field.focus();return null;
}
function addDeviceGroups() {
  var groups=selectedDeviceGroups(),source=deviceSource();
  if((srElement('device-picker').value!=='global' && !sourceNetwork(source)) || !groups.length || ruleWorkspaceLocked())return;
  var priority=readBindingPriority('device-add-priority');if(priority===null)return;
  var target=srElement('device-add-target').value;
  if(!target || !Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL,target)){srShowMsg('请选择有效出口。','err');return;}
  if(confirm('为「'+(source||'全局设备')+'」关联 '+groups.length+' 个规则组，出口：'+(target?(RULE_TARGET_LABEL[target]||target):'尚未指定出口（请编辑）')+'？')){
    ruleApi('save_device_binding',{revision:RULE_RULE_REVISION,source:source,all_devices:srElement('device-picker').value==='global',groups:groups,priority:priority,target:target},toastResp);
  }
}
function editDeviceBinding(group) {
  if(changedBinding() && !confirm('放弃这条绑定尚未保存的修改？'))return;
  FLOW_EDIT_GROUP=group;
  srElement('device-add-groups').hidden=true;srElement('device-add-groups').open=false;srElement('device-fallback').hidden=true;srElement('device-alias-panel').hidden=true;
  srElement('device-group').value=group;
  renderDeviceBinding();
  srElement('device-bound-editor').hidden=false;
  renderRuleFlow();
}
function renderDeviceBinding() {
  var group=srElement('device-group').value;
  srElement('binding-name').textContent='编辑「'+group+'」';
  var bind=(SR_STATE.deviceBindings||[]).find(function(b){return sameDeviceSource(b.source,deviceSource()) && b.group===group;});
  var target=srElement('device-group-target');
  target.innerHTML=deviceOption('','请选择出口（必填）')+Object.keys(RULE_TARGET_LABEL).map(function(key){return deviceOption(key,RULE_TARGET_LABEL[key]);}).join('');
  if(bind && bind.target && !Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL,bind.target))target.innerHTML+=deviceOption(bind.target,'已失效 · '+bind.target);
  target.value=bind?bind.target:'';
  srElement('device-group-priority').value=String(bind && bind.priority || 0);
  bindingBase=bindingFields();
  var members=(SR_STATE.ruleRows||[]).filter(function(r){return !r.source && (r.group||'').trim()===group && !isFinalRule(r);});
  srElement('device-link').disabled=!group || !members.length;
  srElement('device-link').textContent='保存绑定';
  srElement('device-unlink').hidden=!bind;
  srElement('device-group-state').textContent=group?'组内已启用 '+members.filter(function(r){return r.enabled!==false;}).length+' 条目的规则。新增、修改或停用规则会同步到此设备。':'请先在规则组维护中创建包含目的规则的分组。';
}
function inspectDeviceTraffic() {
  var source=deviceSource(), net=sourceNetwork(source);
  if(!net || net.prefix!==net.width)return;
  chooseFlowHost(source.split('/')[0]);
  srElement('rule-flow-destination').focus();
}
function saveDeviceBinding(remove) {
  var picker=srElement('device-picker').value,source=deviceSource(),group=srElement('device-group').value;
  if(ruleWorkspaceLocked() || !picker || (picker!=='global' && !sourceNetwork(source)) || !group)return;
  var target=srElement('device-group-target').value;
  if(!remove && (!target || !Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL,target))){srShowMsg('请选择有效出口。','err');return;}
  var priority=remove?undefined:readBindingPriority('device-group-priority');if(priority===null)return;
  var who=source||'全局设备';
  var ask=remove?'解除 '+who+' 与「'+group+'」的关联？':'将「'+group+'」用于 '+who+'，优先级 '+priority+'，命中后'+(RULE_TARGET_LABEL[target]?'使用「'+RULE_TARGET_LABEL[target]+'」':'沿用各条规则的出口')+'？';
  if(confirm(ask))ruleApi('save_device_binding',{revision:RULE_RULE_REVISION,source:source,all_devices:picker==='global',group:group,target:target,priority:priority,remove:remove},toastResp);
}
function deletePolicyDevice() {
  var source=deviceSource();
  if(ruleWorkspaceLocked() || !sourceNetwork(source))return;
  if(!confirm('删除此设备的全部关联、专属规则、默认出口和自定义名称？在线设备仍会出现在路由器发现列表中。'))return;
  ruleApi('delete_device',{source:source,revision:RULE_RULE_REVISION},function(d){
    if(d.ok || d.saved){deviceDrafts.delete(flowSourceKey(source));activeDraftScope=null;resetDeviceFlow();srElement('device-picker').value='global';renderDevicePolicies();}
    toastResp(d);
  });
}
function saveDeviceAlias() {
  var source=deviceSource(),record=deviceRecord();
  if(ruleWorkspaceLocked() || !sourceNetwork(source))return;
  var field=srElement('device-alias');
  if(field._draftBase!==field._savedValue && !confirm('设备名称已更新为「'+(field._savedValue||'未命名')+'」，仍要保存当前输入吗？'))return;
  ruleApi('save_device_alias',{source:source,key:record?record.alias_key:'',previous:record?record.alias:'',name:field.value.trim()},toastResp);
}
function saveDevicePolicy() {
  if(ruleWorkspaceLocked())return;
  var picker=srElement('device-picker').value,source=deviceSource(),target=srElement('device-target').value;
  if(!picker || (picker!=='global' && !sourceNetwork(source)) || !target)return;
  var who=source||'全局设备';
  if(target!=='inherit' && !Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL,target)){
    srShowMsg('所选出口已失效，请重新选择。','err');return;
  }
  var field=srElement('device-target'), warning=field._draftBase===field._savedValue?'':'默认出口已被修改。';
  if(!confirm(warning+'将「'+who+'」的默认出口设为「'+(RULE_TARGET_LABEL[target]||'恢复默认')+'」？'))return;
  ruleApi('save_device_policy',{revision:RULE_RULE_REVISION,source:source,all_devices:picker==='global',target:target},toastResp);
}

/* ══ Routing rules ═════════════════════════════════════════════════ */
var RULE_TYPE_LABEL = { 'domain': '完整域名', 'domain-suffix': '域名后缀', 'domain-keyword': '域名关键词', 'ip-cidr': 'IP 网段', 'geoip': '地区', 'dst-port': '端口', 'port': '端口', 'final': '最终规则', 'match': '最终规则' };
var RULE_TARGET_LABEL = { 'direct': '直连', 'proxy': '代理', 'reject': '拒绝' };
var RULE_PATTERN_HINT = {
  'domain': 'www.example.invalid',
  'domain-suffix': 'example.invalid',
  'domain-keyword': 'google',
  'ip-cidr': '10.0.0.0/8',
  'geoip': '地区代码，例如 CN、HK、US、JP',
  'dst-port': '443 或 1000-2000',
  'final': '无需填写'
};
var RULE_GEOIP_HINT = '填写地区代码，例如 CN、HK、TW、US、JP、KR、SG；内置数据库覆盖 CN。';
var RULE_BULK_HINT = {
  'domain': 'Clash 格式，每行一条： DOMAIN,host,ACTION',
  'domain-suffix': 'Clash 格式，每行一条： DOMAIN-SUFFIX,domain,ACTION',
  'domain-keyword': 'Clash 格式，每行一条： DOMAIN-KEYWORD,word,ACTION',
  'ip-cidr': 'Clash 格式，每行一条： IP-CIDR,cidr,ACTION',
  'geoip': 'Clash 格式，每行一条： GEOIP,CC,ACTION (e.g. GEOIP,CN,DIRECT)'
};
var RULE_BULK_PLACEHOLDER = {
  'domain': 'DOMAIN,www.example.invalid,PROXY\nDOMAIN,api.example.invalid,PROXY',
  'domain-suffix': 'DOMAIN-SUFFIX,example.invalid,PROXY\nDOMAIN-SUFFIX,media.example.invalid,PROXY\nDOMAIN-SUFFIX,code.example.invalid,PROXY',
  'domain-keyword': 'DOMAIN-KEYWORD,google,PROXY\nDOMAIN-KEYWORD,openai,PROXY',
  'ip-cidr': 'IP-CIDR,10.0.0.0/8,DIRECT\nIP-CIDR,192.168.0.0/16,DIRECT',
  'geoip': 'GEOIP,CN,DIRECT\nGEOIP,HK,DIRECT\nGEOIP,US,PROXY',
  'dst-port': 'DST-PORT,443,PROXY\nDST-PORT,1000-2000,DIRECT',
  'final': 'MATCH,PROXY'
};
function ruleTypeChanged() {
  var t = srElement('rule-type').value;
  var bulk = RULE_EDIT_GROUP !== null || srElement('rule-entry-mode').value === 'bulk';
  srElement('rule-bulk-wrap').style.display = bulk ? '' : 'none';
  srElement('rule-single-wrap').style.display = bulk ? 'none' : '';
  var p = srElement('rule-pattern');
  p.placeholder = RULE_PATTERN_HINT[t] || '';
  p.disabled = (t === 'final');
  srElement('rule-entry-mode').disabled = RULE_EDIT_GROUP !== null;
  if (t === 'final') p.value = '';
  var ta = srElement('rule-batch');
  if (ta) ta.placeholder = RULE_IMPORT_MODE ? '读取链接后，可在这里查看和编辑待导入的规则。' : RULE_BULK_PLACEHOLDER[t] || '';
  var hint = srElement('rule-input-hint');
  if (hint) {
    hint.textContent = bulk
      ? (RULE_EDIT_GROUP !== null ? '编辑整个分组：每行 TYPE,内容,ACTION；设备规则保留 SOURCE,来源, 前缀。每行 ACTION 决定出口，未填写时使用上方默认处理。失效出口请在对应行中修正。' : ((RULE_BULK_HINT[t] || '每行一条 TYPE,内容,ACTION；最终规则为 MATCH,ACTION。') + ' 也可粘贴混合类型的完整规则。'))
      : (t === 'final'
          ? '处理未命中的流量，无需填写匹配内容'
          : (t === 'geoip'
              ? RULE_GEOIP_HINT
              : (RULE_PATTERN_HINT[t] ? ('示例：' + RULE_PATTERN_HINT[t]) : '')));
  }
  // Keep the fallback action editable even when editing a mixed-type group.
  srElement('rule-default-fields').hidden = false;
  ['type','source','priority','dup'].forEach(function(field){srElement('rule-'+field).parentElement.hidden = RULE_EDIT_GROUP !== null || (RULE_IMPORT_MODE && (field==='type' || field==='dup' || field==='source'));});
  srElement('rule-target').parentElement.hidden = false;
  srElement('rule-target-label').textContent = RULE_IMPORT_MODE ? '导入出口（默认代理、指定节点或节点组）' : RULE_EDIT_GROUP !== null ? '默认处理（未填写 ACTION 时使用）' : '处理方式';
  srElement('rule-source').disabled = RULE_EDIT_GROUP !== null;
  if(RULE_IMPORT_MODE && hint)hint.textContent='下方是待追加的规则。每行已写明出口；如需更换导入出口，请选择后重新读取链接，或直接修改各行。';
  var dup = srElement('rule-dup');
  if (dup && dup.parentElement) dup.parentElement.style.display = bulk ? '' : 'none';
}
var RULE_SELECTED_GROUP = null, RULE_LOCAL_GROUPS = [], RULE_WORKSPACE_MODE = 'list';
var RULE_ADD_BASELINE = null, RULE_ROW_BASELINE = null;
var RULE_PRESET_GROUP = null;
var RULE_IMPORT_MODE = false, RULE_IMPORT_PENDING = false, RULE_IMPORT_GENERATION = 0;
function ruleGroupMembers(key) {
  return (SR_STATE.ruleRows || []).filter(function(r){return (r.group || '').trim() === key;});
}
function ruleRowSnapshot() {
  return JSON.stringify(['source','group','type','pattern','target','priority'].map(function(k){return srElement('edit-rule-'+k).value;}).concat([srElement('edit-rule-enabled').checked]));
}
function ruleWorkspaceDirty() {
  return (RULE_ADD_BASELINE !== null && ruleEditorSnapshot() !== RULE_ADD_BASELINE) ||
    (RULE_ROW_BASELINE !== null && ruleRowSnapshot() !== RULE_ROW_BASELINE);
}
function discardRuleDraft() { return !ruleWorkspaceDirty() || confirm('放弃当前未保存的规则修改？'); }
function ruleWorkspaceLocked() {return rulePending || RULE_READ_PENDING || RULE_FLOW_ERROR || !RULE_RULE_REVISION;}
function syncRuleWorkspace() {
  var names=[];
  (SR_STATE.ruleRows || []).forEach(function(r){var name=(r.group || '').trim();if(names.indexOf(name)<0)names.push(name);});
  RULE_LOCAL_GROUPS=RULE_LOCAL_GROUPS.filter(function(name){return names.indexOf(name)<0;});
  names=names.concat(RULE_LOCAL_GROUPS);
  // A refresh must not replace a draft whose group was removed elsewhere.
  if(RULE_SELECTED_GROUP !== null && names.indexOf(RULE_SELECTED_GROUP)<0 && (RULE_ADD_BASELINE !== null || RULE_ROW_BASELINE !== null))names.push(RULE_SELECTED_GROUP);
  if(names.indexOf(RULE_SELECTED_GROUP)<0)RULE_SELECTED_GROUP=names.length?names[0]:null;
  var picker=srElement('rule-group-select'), members=ruleGroupMembers(RULE_SELECTED_GROUP), locked=ruleWorkspaceLocked();
  picker.innerHTML=names.length?names.map(function(name){return '<option value="'+srEsc(name)+'">'+srEsc(name || '未分组')+' · '+ruleGroupMembers(name).length+' 条</option>';}).join(''):'<option value="">暂无分组，请新建</option>';
  picker.value=RULE_SELECTED_GROUP === null?'':RULE_SELECTED_GROUP;picker.disabled=locked || !names.length;
  srElement('rule-bulk-edit').disabled=locked || RULE_SELECTED_GROUP === null;
  var editing=RULE_ADD_BASELINE !== null || RULE_ROW_BASELINE !== null;
  ['enable','disable','delete'].forEach(function(action){srElement('rule-group-'+action).disabled=locked || editing || !members.length;});
  srElement('rule-group-new').disabled=locked;
  srElement('rule-add').disabled=locked || RULE_SELECTED_GROUP === null;
  srElement('rule-import').disabled=locked || !RULE_SELECTED_GROUP;
  var types=[];members.forEach(function(r){var label=RULE_TYPE_LABEL[r.rule_type] || r.rule_type;if(types.indexOf(label)<0)types.push(label);});
  srElement('rule-group-summary').textContent=RULE_SELECTED_GROUP === null?'先新建一个分组，再添加规则。':(RULE_SELECTED_GROUP || '未分组')+' · 共 '+members.length+' 条 · 已启用 '+members.filter(function(r){return r.enabled!==false;}).length+' 条'+(types.length?' · '+types.join(' / '):' · 添加第一条规则后保存');
  srElement('rule-list-panel').hidden=RULE_WORKSPACE_MODE!=='list' && !RULE_IMPORT_MODE;
  ['rule-add-panel','rule-edit-panel'].forEach(function(id){var el=srElement(id);if(el.querySelectorAll)Array.prototype.forEach.call(el.querySelectorAll('input,select,textarea,button'),function(input){input.disabled=locked;});});
  if(!locked){ruleTypeChanged();editRuleTypeChanged();}
  srElement('rule-import-fetch').disabled=locked || RULE_IMPORT_PENDING;
  if(RULE_PRESET_GROUP!==null)renderRulePreset();
}
function changeRuleGroup(key,scope) {
  if(ruleWorkspaceLocked() || !discardRuleDraft()) {srElement('rule-group-select').value=RULE_SELECTED_GROUP || '';return false;}
  resetRuleEditor();RULE_SELECTED_GROUP=key;srElement('rule-group').value=key;
  srElement('rule-search').value='';srElement('rule-filter-target').value='';srElement('rule-filter-state').value='';srElement('rule-filter-scope').value=scope||'';
  renderRules();return true;
}
function changeRuleWorkspaceMode(mode) {
  if(ruleWorkspaceLocked())return;
  if(!discardRuleDraft())return;
  resetRuleEditor();
  if(mode==='import')startRuleImport();
  else if(mode==='bulk') {
    if(ruleGroupMembers(RULE_SELECTED_GROUP).length)editRuleGroup(encodeURIComponent(RULE_SELECTED_GROUP));
    else {startRuleAddition();RULE_WORKSPACE_MODE='bulk';srElement('rule-entry-mode').value='bulk';ruleTypeChanged();RULE_ADD_BASELINE=ruleEditorSnapshot();}
  }
  renderRules(false);
}
function closeRuleCreateMenu() {
  srElement('rule-create-menu').hidden=true;
  srElement('rule-create-toggle').setAttribute('aria-expanded','false');
}
var ruleCreateMethod = 'blank';
function openRuleGroupCreator(method) {
  if(ruleWorkspaceLocked())return;
  ruleCreateMethod = method || 'blank';
  var preset = ruleCreateMethod === 'preset';
  srElement('rule-preset-panel').hidden = !preset;
  srElement('rule-group-creator').hidden = preset;
  srElement('rule-create-menu').hidden=false;
  srElement('rule-create-toggle').setAttribute('aria-expanded','true');
  srElement(preset ? 'rule-preset' : 'rule-new-group-name').focus();
}
function createRuleGroup() {
  if(ruleWorkspaceLocked())return;
  var name=srElement('rule-new-group-name').value.trim();
  if(!name){srShowMsg('请输入分组名称。','err');return;}
  if(ruleGroupMembers(name).length || RULE_LOCAL_GROUPS.indexOf(name)>=0){srShowMsg('分组已存在，请从上方选择。','err');return;}
  if(!discardRuleDraft())return;
  resetRuleEditor();RULE_LOCAL_GROUPS.push(name);RULE_SELECTED_GROUP=name;
  srElement('rule-search').value='';srElement('rule-filter-target').value='';srElement('rule-filter-state').value='';srElement('rule-filter-scope').value='';
  closeRuleCreateMenu();srElement('rule-group-creator').hidden=true;srElement('rule-new-group-name').value='';
  if(ruleCreateMethod === 'url')startRuleImport();
  else startRuleAddition();
}
function renderRuleAddMethod(method) {
  srElement('rule-add-methods').hidden=RULE_EDIT_GROUP!==null;
  srElement('rule-input-fields').hidden=method==='preset';
  srElement('rule-preset-panel').hidden=method!=='preset';
  [['rule-method-manual','manual'],['rule-import','url'],['rule-method-preset','preset']].forEach(function(pair){
    srElement(pair[0]).setAttribute('aria-pressed',String(pair[1]===method));
  });
}
function startRulePresetAddition() {
  if(ruleWorkspaceLocked() || RULE_SELECTED_GROUP===null || !discardRuleDraft())return;
  resetRuleEditor();RULE_PRESET_GROUP=RULE_SELECTED_GROUP;
  srElement('rule-add-panel').hidden=false;
  srElement('rule-editor-state').textContent='向「'+(RULE_SELECTED_GROUP||'未分组')+'」添加规则';
  renderRuleAddMethod('preset');renderRulePreset();RULE_ADD_BASELINE=ruleEditorSnapshot();
}
function startRuleAddition() {
  if(ruleWorkspaceLocked() || RULE_SELECTED_GROUP === null || !discardRuleDraft())return;
  resetRuleEditor();srElement('rule-add-panel').hidden=false;renderRuleAddMethod('manual');
  srElement('rule-editor-state').textContent='向「'+(RULE_SELECTED_GROUP || '未分组')+'」添加规则';
  srElement('rule-group').value=RULE_SELECTED_GROUP;
  var proxy=ruleGroupMembers(RULE_SELECTED_GROUP).filter(function(r){return proxyLikeTarget(r.target);});
  srElement('rule-target').value=proxy.length?proxy[0].target:'proxy';
  ruleTypeChanged();RULE_ADD_BASELINE=ruleEditorSnapshot();renderRules(false);srElement('rule-type').focus();
}
function startRuleImport() {
  if(ruleWorkspaceLocked() || RULE_SELECTED_GROUP === null || !discardRuleDraft())return;
  if(!RULE_SELECTED_GROUP){srShowMsg('请先新建或选择具名规则组。','err');return;}
  resetRuleEditor();startRuleAddition();
  RULE_IMPORT_MODE=true;RULE_WORKSPACE_MODE='import';renderRuleAddMethod('url');
  srElement('rule-entry-mode').value='bulk';srElement('rule-dup').value='skip';
  srElement('rule-import-panel').hidden=false;
  srElement('rule-editor-state').textContent='向「'+(RULE_SELECTED_GROUP || '未分组')+'」添加规则';
  ruleTypeChanged();RULE_ADD_BASELINE=ruleEditorSnapshot();renderRules(false);srElement('rule-import-url').focus();
}
function fetchRuleList() {
  if(!RULE_IMPORT_MODE || ruleWorkspaceLocked() || RULE_IMPORT_PENDING)return;
  var url=srElement('rule-import-url').value.trim();
  try {var parsed=new URL(url);if(!/^https?:$/.test(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password || /\s/.test(url))throw new Error();}
  catch(e){srShowMsg('请填写有效的 HTTP 或 HTTPS 规则列表链接。','err');return;}
  if(srElement('rule-batch').value.trim() && !confirm('重新读取会替换下方待导入内容，已保存的规则保持不变。继续？'))return;
  var generation=++RULE_IMPORT_GENERATION,draft=ruleEditorSnapshot();
  RULE_IMPORT_PENDING=true;srElement('rule-import-fetch').disabled=true;
  srElement('rule-import-state').textContent='正在读取规则链接…';
  function finish(d) {
    if(generation!==RULE_IMPORT_GENERATION || !RULE_IMPORT_MODE)return;
    RULE_IMPORT_PENDING=false;srElement('rule-import-fetch').disabled=ruleWorkspaceLocked();
    if(draft!==ruleEditorSnapshot()) {srElement('rule-import-state').textContent='读取期间内容或出口已变化，未覆盖草稿；请重新读取。';return;}
    if(!d || d.ok===false || typeof d.text!=='string' || !d.text.trim()) {srElement('rule-import-state').textContent=d&&(d.msg||d.error_message)||'读取失败，请检查链接后重试；现有规则未改变。';return;}
    var editor = srElement('rule-batch');
    editor.value = d.text;
    editor.focus();
    editor.selectionStart = editor.selectionEnd = 0;
    editor.scrollTop = 0;
    var skipped=Object.keys(d.skipped||{}).map(function(kind){return kind+' × '+d.skipped[kind];});
    srElement('rule-import-state').textContent='已读取 '+Number(d.count||0)+' 条待导入规则。'+(skipped.length?'已跳过不支持的类型：'+skipped.join('、')+'。':'')+'请检查预览后确认添加。';
    previewRuleChanges();
  }
  srApi('fetch_rule_list',{url:url,target:srElement('rule-target').value,group:RULE_SELECTED_GROUP},finish,finish,40000);
}
function cancelRuleRowEditor() {if(discardRuleDraft()){resetRuleEditor();renderRules(false);}}

var RULE_COLLAPSED = Object.create(null);
var SR_RULE_NODES = SR_RULE_NODES || [];
function setRuleNodes(nodes) {
  SR_RULE_NODES = Array.isArray(nodes) ? nodes.filter(function(n){return n && typeof n.id==='string' && /^[0-9a-f]{64}$/.test(n.id) && typeof n.name==='string';}) : [];
  rebuildRuleTargets();
}
function rebuildRuleTargets() {
  Object.keys(RULE_TARGET_LABEL).forEach(function(k){if(k.indexOf('group:')===0 || k.indexOf('node:')===0)delete RULE_TARGET_LABEL[k];});
  SR_RULE_NODES.forEach(function(n){RULE_TARGET_LABEL['node:'+n.id]='节点 · '+n.name;});
  (SR_PROXY_GROUPS||[]).forEach(function(g){RULE_TARGET_LABEL['group:'+g.name]='分组 · '+g.name;});
  var values=['proxy','direct','reject'].concat(SR_RULE_NODES.map(function(n){return 'node:'+n.id;})).concat((SR_PROXY_GROUPS||[]).map(function(g){return 'group:'+g.name;}));
  ['rule-target','edit-rule-target','rule-filter-target','device-target','device-add-target','preset-route-target'].forEach(function(id){
    var el=srElement(id);if(!el)return;var old=el.value;
    var prefix=id==='device-add-target'?'<option value="">请选择出口（必填）</option>':id==='rule-filter-target'?'<option value="">全部处理方式</option>':id==='device-target'?'<option value="inherit">清除自定义默认出口</option>':id==='preset-route-target'?'<option value="">使用预设默认</option>':'';
    el.innerHTML=prefix+values.map(function(t){return '<option value="'+srEsc(t)+'">'+srEsc(RULE_TARGET_LABEL[t])+'</option>';}).join('');
    var current=el.options?Array.from(el.options):[];
    if(current.some(function(o){return o.value===old;}))el.value=old;
    else if(old && el.options && (old.indexOf('group:')===0 || old.indexOf('node:')===0)){var o=document.createElement('option');o.value=old;o.textContent='已失效 · '+old;el.appendChild(o);el.value=old;}
  });
}
function ruleTargetKnown(action) {
  return Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL, action) || (typeof action==='string' && (action.indexOf('group:')===0 || action.indexOf('node:')===0));
}
var RULE_PRESETS=[];
function loadRulePresets(presets) {
  var select=srElement('rule-preset'),previous=select.value;
  RULE_PRESETS=Array.isArray(presets)?presets.filter(function(p){return p && /^[a-z_]+$/.test(p.id) && typeof p.group==='string' && typeof p.title==='string' && Array.isArray(p.domains) && p.domains.every(function(d){return typeof d==='string';}) && Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL,p.target);}):[];
  select.innerHTML=RULE_PRESETS.length?RULE_PRESETS.map(function(p){return '<option value="'+srEsc(p.id)+'">'+srEsc(p.title)+'</option>';}).join(''):'<option value="">暂无预设</option>';
  select.value=RULE_PRESETS.some(function(p){return p.id===previous;})?previous:RULE_PRESETS.length?RULE_PRESETS[0].id:'';
  renderRulePreset();
}
function rulePresetGroup(p){return RULE_PRESET_GROUP===null?p.group:RULE_PRESET_GROUP;}
function selectedRulePreset(){return RULE_PRESETS.filter(function(p){return p.id===srElement('rule-preset').value;})[0];}
function renderRulePreset() {
  var p=selectedRulePreset(),unavailable=RULE_FLOW_ERROR||!RULE_RULE_REVISION||!p,busy=!!rulePending||RULE_READ_PENDING;
  srElement('rule-preset').disabled=unavailable||busy;
  srElement('rule-preset-add').disabled=true;
  ['view','enable','disable'].forEach(function(id){srElement('rule-preset-'+id).hidden=true;srElement('rule-preset-'+id).disabled=unavailable||busy;});
  if(unavailable){srElement('rule-preset-state').textContent=RULE_FLOW_ERROR?'规则加载失败，请先刷新。':'预设尚不可用，请加载规则或通过软件中心安装完整新版插件。';srElement('rule-preset-domains').textContent='';srElement('rule-preset-target').textContent='';srElement('rule-preset-description').textContent='';return;}
  var destination=rulePresetGroup(p);
  var rows=SR_STATE.ruleRows||[],members=rows.filter(function(r){return (r.group||'').trim()===destination;}),enabled=members.filter(function(r){return r.enabled!==false;}).length;
  var ruleType=p.rule_type||'domain-suffix';
  var missing=p.domains.filter(function(d){return !rows.some(function(r){return (RULE_PRESET_GROUP!==null ? (r.group||'')===destination : (ruleType!=='domain' || r.group===destination)) && !r.source && r.rule_type===ruleType && String(r.pattern).replace(/^\.+/,'').toLowerCase()===d.toLowerCase();});}).length;
  srElement('rule-preset-target').textContent='默认：'+RULE_TARGET_LABEL[p.target];
  srElement('rule-preset-description').textContent='添加到「'+(destination||'未分组')+'」，初始未启用。'+(p.description||'');
  srElement('rule-preset-state').textContent=(members.length?'当前组 '+members.length+' 条 · 已启用 '+enabled+' 条':'尚未添加 · 未启用')+' · 预设共 '+p.domains.length+' 个域名'+(missing?' · 可添加 '+missing+' 条':' · 域名规则已存在，不重复添加');
  srElement('rule-preset-domains').textContent=p.domains.map(function(d){return ruleType.toUpperCase()+','+d+','+p.target.toUpperCase();}).join('\n');
  srElement('rule-preset-add').textContent='添加 '+missing+' 条到「'+(destination||'未分组')+'」（未启用）';
  srElement('rule-preset-add').disabled=busy||!missing;
  srElement('rule-preset-view').hidden=!members.length;
  srElement('rule-preset-enable').hidden=enabled===members.length;
  srElement('rule-preset-disable').hidden=!enabled;
}
function addRulePreset() {
  var p=selectedRulePreset();if(!p||RULE_FLOW_ERROR||!RULE_RULE_REVISION)return;
  var destination=rulePresetGroup(p), presetDraft=ruleEditorSnapshot();
  ruleApi('add_rule_preset',Object.assign(Object.assign({preset:p.id,expected_revision:RULE_RULE_REVISION},RULE_PRESET_GROUP===null?{}:{group:destination}),srElement('preset-route-target').value?{target:srElement('preset-route-target').value}:{}),function(d){
    toastResp(d);
    if(d.ok && presetDraft===ruleEditorSnapshot() && ruleGroupMembers(destination).length){
      resetRuleEditor();changeRuleGroup(destination);
      srElement('rule-preset-panel').hidden=true;
      closeRuleCreateMenu();
      srElement('rule-workspace').scrollIntoView({behavior:'smooth',block:'start'});
    }
  });
}
function viewRulePreset() {
  var p=selectedRulePreset();if(!p || !changeRuleGroup(rulePresetGroup(p)))return;
  srElement('rule-workspace').scrollIntoView({behavior:'smooth',block:'start'});
}
function toggleRulePreset(enabled) {
  var p=selectedRulePreset();if(!p||RULE_FLOW_ERROR)return;
  setRuleGroupEnabled(encodeURIComponent(rulePresetGroup(p)),enabled);
}
function groupRulesForDisplay(rules) {
  var buckets = [];
  rules.forEach(function(r, i) {
    var g = (r.group || '').trim(), last = buckets[buckets.length - 1];
    if (!last || last.name !== g || rulePhase(r) !== rulePhase(rules[last.indices[0]])) {
      last = {name:g, indices:[]}; buckets.push(last);
    }
    last.indices.push(i);
  });
  return buckets;
}
function isFinalRule(r) { var kind=String(r.rule_type).toLowerCase(); return kind==='match'||kind==='final'; }
function rulePhase(r) {return isFinalRule(r) ? (r.source ? 2 : 3) : (r.source ? 0 : 1);}
function orderedRules(rules) {
  return rules.slice().sort(function(a,b) {
    return rulePhase(a) - rulePhase(b) || Number(b.priority || 0) - Number(a.priority || 0) || Number(a.index) - Number(b.index);
  });
}
var RULE_FLOW_SELECTED = 0, RULE_FLOW_ERROR = false, RULE_FLOW_CLIENT = '';
var RULE_TRACE=null,RULE_TRACE_PENDING=false,RULE_TRACE_GENERATION=0,RULE_RULE_REVISION=null,RULE_FLOW_FILTER=null,RULE_TRACE_CONTEXT=null;
var RULE_READ_PENDING=false,RULE_LOAD_GENERATION=0,RULE_SNAPSHOT_MODE=null;
function ruleTraceContext(){var s=SR_STATE.status||{};return JSON.stringify([s.proxy_mode,s.active_node,s.node_count,s.running,s.transparent_proxy,s.cn_ipset_direct]);}
// Used only for local diagram previews; Rust validates saved selectors.
function sourceNetwork(text) {
  var parts=String(text).trim().split('/');if(parts.length>2 || !parts[0])return null;
  var address=parts[0].toLowerCase(), width=32, value=BigInt(0), fields;
  function v4(value) {var a=value.split('.');return a.length===4 && a.every(function(v){return /^\d{1,3}$/.test(v)&&Number(v)<=255;}) ? a.map(Number) : null;}
  if(address.indexOf(':')<0) {fields=v4(address);if(!fields)return null;fields.forEach(function(v){value=(value<<BigInt(8))+BigInt(v);});}
  else {
    width=128;if(address.indexOf('.')>=0) {var at=address.lastIndexOf(':'), tail=v4(address.slice(at+1));if(!tail)return null;address=address.slice(0,at+1)+((tail[0]<<8)+tail[1]).toString(16)+':'+((tail[2]<<8)+tail[3]).toString(16);}
    var halves=address.split('::');if(halves.length>2)return null;
    var left=halves[0] ? halves[0].split(':') : [], right=halves.length===2&&halves[1] ? halves[1].split(':') : [];
    if(halves.length===1)fields=left;else {var missing=8-left.length-right.length;if(missing<1)return null;fields=left.concat(Array(missing).fill('0'),right);}
    if(fields.length!==8||!fields.every(function(v){return /^[a-f0-9]{1,4}$/.test(v);}))return null;
    fields.forEach(function(v){value=(value<<BigInt(16))+BigInt(parseInt(v,16));});
  }
  var prefix=parts.length===1 ? width : (/^\d+$/.test(parts[1]) ? Number(parts[1]) : -1);
  if(prefix<0 || prefix>width)return null;
  if(width===128 && (value>>BigInt(32))===BigInt(65535) && prefix>=96) {width=32;value&=BigInt('4294967295');prefix-=96;}
  return {value:value,width:width,prefix:prefix};
}
function sourceContains(scope,client) {
  var net=sourceNetwork(scope), ip=sourceNetwork(client);
  return !!(net&&ip&&net.width===ip.width&&net.prefix<=ip.prefix&&(net.value>>BigInt(net.width-net.prefix))===(ip.value>>BigInt(net.width-net.prefix)));
}
// Hosts can be simulated; a configured subnet describes a branch, not a client IP.
function hasDeviceAssignment(source) {
  if(!source)return false;
  function applies(r){return r.source && sourceContains(r.source,source);}
  return (SR_STATE.deviceBindings||[]).some(applies) || (SR_STATE.ruleRows||[]).some(function(r){return r.enabled!==false && applies(r);});
}
function flowHost(source) {
  var net=sourceNetwork(source);
  return net && net.prefix===net.width ? source.split('/')[0] : '';
}
function sameDeviceSource(a,b) {
  if(a===b)return true;
  var x=sourceNetwork(a), y=sourceNetwork(b);
  return !!(x && y && x.width===y.width && x.prefix===y.prefix && sourceContains(a,b));
}
function flowSourceKey(source) {
  var net=sourceNetwork(source);
  return net?[net.width,net.prefix,net.value>>BigInt(net.width-net.prefix)].join('/'):'';
}
function flowDeviceChoices() {
  var hosts=new Map(),keys=new Set();
  function add(source,name){
    var ip=flowHost(source);
    if(!ip)return;
    var key=flowSourceKey(ip);
    if(keys.has(key))return;
    keys.add(key);hosts.set(ip,name);
  }
  (SR_STATE.devices||[]).forEach(function(d){add(d.source,d.alias||d.host||'');});
  (SR_STATE.deviceBindings||[]).concat(SR_STATE.ruleRows||[]).forEach(function(r){if(r.source)add(r.source,'');});
  return hosts;
}
function syncFlowDevicePicker() {
  var picker=srElement('rule-flow-device'),before=picker.value,hosts=flowDeviceChoices();
  var options='<option value="">全局设备 · 配置总览</option>';
  hosts.forEach(function(name,ip){
    options+='<option value="'+srEsc(ip)+'">'+srEsc(name ? name+' · '+ip : ip)+'</option>';
  });
  picker.innerHTML=options+'<option value="manual">手动输入设备 IP</option><option value="global">仅模拟全局规则</option>';
  if(before && before!=='manual' && before!=='global' && !hosts.has(before)){
    var equivalent=Array.from(hosts.keys()).find(function(ip){return sameDeviceSource(ip,before);});
    if(equivalent){picker.value=equivalent;return;}
    picker.value='';changeFlowDevice();return;
  }
  picker.value=before || '';
}
function changeFlowDevice() {
  FLOW_EDIT_NETWORK='';FLOW_EDIT_GROUP=null;
  var value=srElement('rule-flow-device').value;
  var manual=value==='manual',field=srElement('rule-flow-client');
  srElement('rule-flow-manual').hidden=!manual;
  field.value=manual || value==='global'?'':value;
  RULE_FLOW_SELECTED=0;
  invalidateRuleTrace(value?'已切换模拟来源，输入网站或目的 IP 后检查。':'配置总览。请选择设备后模拟。');
}
function chooseFlowHost(ip) {
  var picker=srElement('rule-flow-device');
  picker.value=ip;
  if(picker.value!==ip){
    var option=Array.from(picker.options).find(function(o){return sameDeviceSource(o.value,ip);});
    picker.value=option?option.value:'manual';
  }
  changeFlowDevice();
  if(picker.value==='manual')srElement('rule-flow-client').value=ip;
  previewDeviceFlow();
}
var FLOW_EDIT_NETWORK='',FLOW_EDIT_GROUP=null;
function setFlowEditorSource(source) {
  var picker=srElement('device-picker');
  var option=source && Array.from(picker.options).find(function(o){return sameDeviceSource(o.value,source);});
  picker.value=source?(option?option.value:'manual'):'global';
  selectPolicyDevice();
  if(picker.value==='manual'){srElement('device-source').value=source;updateDevicePolicySelection();}
}
function syncSimulatedSource() {
  var value=srElement('rule-flow-device').value, source=srElement('rule-flow-client').value.trim();
  if(value==='global'||sourceNetwork(source))setFlowEditorSource(value==='global'?'':source);
  else if(value==='manual'){
    srElement('device-picker').value='manual';
    selectPolicyDevice();
    srElement('device-source').value=source;
    updateDevicePolicySelection();
  }
  else setFlowEditorSource('');
}
function pickMapSource(source) {
  if(source && !flowHost(source)){
    srElement('rule-flow-device').value='manual';changeFlowDevice();
    FLOW_EDIT_NETWORK=source;renderRuleFlow();
    srElement('rule-flow-query-state').textContent='正在配置此网段。模拟时请输入网段内的设备 IP。';
  }else if(source)chooseFlowHost(flowHost(source));else chooseGlobalFlow();
  setFlowEditorSource(source);
}
function syncEditorToMap() {
  var value=srElement('device-picker').value,source=deviceSource();
  if(value==='global')chooseGlobalFlow();
  else if(flowHost(source))chooseFlowHost(flowHost(source));
  else if(sourceNetwork(source))pickMapSource(source);
  else resetDeviceFlow();
}
function openMapGroup(index) {
  var rule=(SR_STATE.ruleRows||[]).find(function(r){return Number(r.index)===Number(index);});
  if(!rule)return;
  var name=(rule.group||'').trim(), source=FLOW_EDIT_NETWORK||RULE_FLOW_CLIENT;
  FLOW_EDIT_GROUP=null;
  if((source || srElement('device-picker').value==='global') && name && (SR_STATE.ruleRows||[]).some(function(r){return !r.source && !isFinalRule(r) && (r.group||'').trim()===name;})){
    setFlowEditorSource(source);
      var bound=(SR_STATE.deviceBindings||[]).some(function(link){return sameDeviceSource(source,link.source)&&link.group===name;});
    if(bound){
      FLOW_EDIT_GROUP=name;
      editDeviceBinding(name);
      srElement('device-group-target').focus();
    }else{
      var boxes=srElement('device-available-groups').querySelectorAll('input');
      Array.from(boxes).forEach(function(box){if(box.value===name)box.checked=true;});
      switchDevicePanel('add');updateDeviceAddCount();
      srElement('device-add-selected').focus();
    }
    renderRuleFlow();
  }else if(changeRuleGroup(name,rule.source?'device':'global')){
    srElement('rule-workspace').scrollIntoView({behavior:'smooth',block:'start'});
  }
}
function pickMapExit(target) {
  if(ruleWorkspaceLocked() || FLOW_EDIT_GROUP===null || srElement('device-group').value!==FLOW_EDIT_GROUP)return;
  var select=srElement('device-group-target');
  if(!Array.from(select.options).some(function(o){return o.value===target;}))return;
  select.value=target;srElement('device-bound-editor').hidden=false;srElement('device-link').focus();
}

// Connections represent possible routes. Per-source membership on each edge
// distinguishes two devices using the same group with different exits.
var sharedMapCache;
function sharedFlowGraph(all,mode,activeStages,winner) {
  var choice=srElement('rule-flow-device').value;
  var selected=FLOW_EDIT_NETWORK||RULE_FLOW_CLIENT||(choice==='global'?'':null);
  // Router status polls reuse the topology until a snapshot or selection changes.
  var inputs=[all,mode,selected,FLOW_EDIT_GROUP,winner,RULE_TRACE,SR_STATE.devices,SR_STATE.deviceBindings,SR_STATE.globalBindings,SR_STATE.compiledTargets,SR_STATE.nodes,(SR_STATE.status||{}).active_node,SR_PROXY_GROUPS,SR_RULE_NODES];
  if(sharedMapCache && inputs.every(function(value,i){return sharedMapCache.inputs[i]===value;}))return sharedMapCache.html;
  var names=new Map(),scopes=new Map([['','']]),groups=new Map(),exits=new Map(),edges=new Map();
  (SR_STATE.devices||[]).forEach(function(d){
    var key=flowSourceKey(d.source);
    names.set(key,d.alias||d.host||'');
    if(d.alias)scopes.set(key,flowHost(d.source)||d.source);
  });
  var ordered=orderedFlowRules(all);
  (SR_STATE.deviceBindings||[]).concat(all).forEach(function(r){if(r.source)scopes.set(flowSourceKey(r.source),flowHost(r.source)||r.source);});
  if(selected!==null)scopes.set(flowSourceKey(selected),selected);
  var sources=Array.from(scopes.values()), original=new Map(all.map(function(r){return [Number(r.index),r];}));
  function groupKey(stage,scope){
    if(!stage.indices)return 'stage:'+stage.kind;
    var name=String(original.get(stage.indices[0]).group||'').trim();
    // Unnamed rules have no shared group identity across sources or matching phases.
    return name?'group:'+name:'unnamed:'+stage.kind+':'+flowSourceKey(scope);
  }
  function edge(left,a,b,scope,hit){
    var id=[left,a,b].join('/'),e=edges.get(id);
    if(!e){e={left:left,a:a,b:b,related:false,hit:false};edges.set(id,e);}
    if(scope===selected)e.related=true;
    if(hit)e.hit=true;
  }
  var matched=winner>=0?activeStages[winner]:null;
  var trace=matched && RULE_TRACE.trace;
  var matchedStep=trace && trace.steps.find(function(step){return step.outcome==='matched';});
  var matchedSource=flowSourceKey(matched && matched.kind==='device_default'?selected:matchedStep && matchedStep.source);
  // Keep source positions stable while laying groups out in the selected device's match order.
  var scopeOrder=sources.map(function(_,i){return i;});
  if(selected!==null)scopeOrder.sort(function(a,b){return Number(sources[b]===selected)-Number(sources[a]===selected);});
  scopeOrder.forEach(function(i){
    var scope=sources[i];
    // Draw the policies owned by this source.
    var stages=ruleFlowModel(all,mode,scope,ordered,true);
    var ownsMatch=!!matched && flowSourceKey(scope)===matchedSource;
    stages.forEach(function(stage){
      var key=groupKey(stage,scope),g=groups.get(key);
      var unnamedFinal=key.indexOf('unnamed:')===0 && (stage.kind==='global_final'||stage.kind==='device_final');
      if(!g){g={index:groups.size,title:unnamedFinal?stage.phaseLabel:stage.title,ruleIndex:stage.indices?stage.indices[0]:null,phases:new Set(),enabled:false,selected:false};groups.set(key,g);}
      g.phases.add(unnamedFinal?'前序规则均未命中时使用':stage.phaseLabel||stage.title);
      if(scope===selected && stage.bindingPriority!=null)g.bindingPriority=stage.bindingPriority;
      if(stage.disabled)return;
      g.enabled=true;
      var hit=ownsMatch && stage.kind===matched.kind &&
        (stage.indices?stage.indices.includes(trace.matched_rule):!matched.indices);
      if(hit || (!matched && scope===selected && (FLOW_EDIT_GROUP===null||key==='group:'+FLOW_EDIT_GROUP)))g.selected=true;
      edge(true,i,g.index,scope,hit);
      stage.targets.forEach(function(action){
        if(!exits.has(action))exits.set(action,exits.size);
        edge(false,g.index,exits.get(action),scope,hit && RULE_TRACE.trace.action===action);
      });
    });
  });
  var height=Math.max(sources.length,groups.size,exits.size)*68;
  function center(index,count){return (height-(count-1)*68)/2+index*68;}
  var paths=Array.from(edges.values(),function(e){
    var x=e.left?220:670, end=e.left?330:780,mid=(x+end)/2;
    var y=center(e.a,e.left?sources.length:groups.size),dest=center(e.b,e.left?groups.size:exits.size);
    var style=e.hit?'selected':selected===null?'':e.related&&!matched?'related':'dim';
    return '<path data-from="'+(e.left?'s':'g')+e.a+'" data-to="'+(e.left?'g':'e')+e.b+'" class="'+style+'" d="M '+x+' '+y+' C '+mid+' '+y+', '+mid+' '+dest+', '+end+' '+dest+'"/>';
  }).join('');
  function box(kind,y,title,sub,cls,button){
    var tag=button?'button':'div';
    return '<'+tag+(button?' type="button" '+button:'')+' class="map-node '+kind+cls+'" style="top:'+(y-27)+'px" title="'+srEsc(title+(sub?' · '+sub:''))+'"><strong>'+srEsc(title)+'</strong><small>'+srEsc(sub)+'</small></'+tag+'>';
  }
  var nodes=sources.map(function(scope,i){
    var name=names.get(flowSourceKey(scope));
    return box('source',center(i,sources.length),name||scope||'全局设备',scope?(name?scope:'配置此来源'):'全局规则',scope===selected?' selected':'','data-source="'+srEsc(scope)+'" onclick="pickMapSource(this.dataset.source)"');
  }).join('');
  groups.forEach(function(g){
    nodes+=box('group',center(g.index,groups.size),g.title,g.enabled?(g.bindingPriority!=null?'关联优先级 '+g.bindingPriority+' · ':'')+Array.from(g.phases).join(' / '):'已停用',g.selected?' selected':g.enabled?'':' inactive',g.ruleIndex==null?'':'data-index="'+g.ruleIndex+'" onclick="openMapGroup(this.dataset.index)"');
  });
  exits.forEach(function(i,action){
    var sub='';
    if(action==='proxy'){var node=(SR_STATE.nodes||[])[(SR_STATE.status||{}).active_node];sub=node?node.name:'当前所选节点';}
    else if(action.indexOf('group:')===0){var g=(SR_PROXY_GROUPS||[]).find(function(g){return g.name===action.slice(6);});sub=g&&g.selected_node||'';}
    nodes+=box('outlet',center(i,exits.size),RULE_TARGET_LABEL[action]||action,sub,matched && RULE_TRACE.trace.action===action?' selected':'',FLOW_EDIT_GROUP===null?'':'data-target="'+srEsc(action)+'" onclick="pickMapExit(this.dataset.target)"');
  });
  var html='<div class="map-head"><span>来源</span><span>规则组 / 策略</span><span>出口</span></div><div class="routing-map" aria-label="设备与全局规则共享分流图" style="height:'+height+'px"><svg class="flow-lines" aria-hidden="true" viewBox="0 0 1000 '+height+'" preserveAspectRatio="none">'+paths+'</svg>'+nodes+'</div>';
  sharedMapCache={inputs:inputs,html:html};
  return html;
}
function chooseGlobalFlow() {
  srElement('rule-flow-device').value='global';
  changeFlowDevice();
}

function invalidateRuleTrace(message) {
  RULE_TRACE_GENERATION++; RULE_TRACE=null; RULE_TRACE_PENDING=false;
  var input=srElement('rule-flow-client').value.trim();
  RULE_FLOW_CLIENT=input && input.indexOf('/')<0 && sourceNetwork(input) ? input : '';
  srElement('rule-flow-check').disabled=false;
  srElement('rule-flow-check').textContent='检查分流';
  srElement('rule-flow-query-state').textContent=message || '输入已更改，请重新检查。';
  renderRuleFlow();
}
function previewDeviceFlow() {
  var text=srElement('rule-flow-client').value.trim();
  if(text && (!sourceNetwork(text) || text.indexOf('/')>=0)) {srShowMsg('设备来源请填写单个 IPv4 或 IPv6 地址。','err');return;}
  invalidateRuleTrace('已显示此来源可用的路径，输入目的地址可检查具体命中。');
  RULE_FLOW_SELECTED=0;renderRuleFlow();
}
function resetDeviceFlow() {
  FLOW_EDIT_NETWORK='';FLOW_EDIT_GROUP=null;
  srElement('rule-flow-device').value='';srElement('rule-flow-manual').hidden=true;
  ['rule-flow-client','rule-flow-destination','rule-flow-port','rule-flow-ip'].forEach(function(id){srElement(id).value='';});
  srElement('rule-flow-resolve').checked=true;RULE_FLOW_SELECTED=0;
  invalidateRuleTrace('连线表示可能的出口，每次匹配只选一个。输入目的地址可检查命中路径；点击规则组可查看规则。');
}
function traceRuleFlow() {
  if(RULE_TRACE_PENDING)return;
  if(RULE_FLOW_ERROR||RULE_READ_PENDING||rulePending){srShowMsg('请等待规则保存和刷新完成，再检查分流。','err');return;}
  var destination=srElement('rule-flow-destination').value.trim(), source=srElement('rule-flow-client').value.trim(), ip=srElement('rule-flow-ip').value.trim(), port=srElement('rule-flow-port').value.trim();
  var choice=srElement('rule-flow-device').value;
  if(!choice || (choice==='manual' && !source)){
    srElement('rule-flow-query-state').textContent='请选择一台模拟设备、手动输入 IP，或选择「仅模拟全局规则」。';
    srElement(choice==='manual'?'rule-flow-client':'rule-flow-device').focus();return;
  }
  if(choice!=='manual')source=choice==='global'?'':choice;
  if(!destination){srShowMsg('请填写要检查的网站或目的 IP。','err');return;}
  if([source,ip].some(function(v){return v && (!sourceNetwork(v)||v.indexOf('/')>=0);})){srShowMsg('设备和目的 IP 请填写单个 IPv4 或 IPv6 地址。','err');return;}
  if(port && (!/^\d+$/.test(port)||Number(port)<1||Number(port)>65535)){srShowMsg('端口必须为 1–65535。','err');return;}
  RULE_TRACE=null;RULE_TRACE_PENDING=true;RULE_FLOW_CLIENT=source;
  RULE_TRACE_CONTEXT=ruleTraceContext();
  var generation=++RULE_TRACE_GENERATION;
  srElement('rule-flow-check').disabled=true;srElement('rule-flow-check').textContent='正在检查…';
  srElement('rule-flow-query-state').textContent='按保存的配置检查中，DNS 解析最多等待 12 秒…';
  renderRuleFlow();
  var body={destination:destination,source:source,destination_ip:ip,resolve_dns:srElement('rule-flow-resolve').checked};
  if(port)body.port=Number(port);
  if(RULE_RULE_REVISION)body.expected_revision=RULE_RULE_REVISION;
  function finish(){if(generation!==RULE_TRACE_GENERATION)return false;RULE_TRACE_PENDING=false;srElement('rule-flow-check').disabled=false;srElement('rule-flow-check').textContent='检查分流';return true;}
  srApi('trace_route',body,function(d){
    if(!finish())return;
    if(!d || d.ok!==true || !d.trace || !ruleTargetKnown(d.trace.action) || !Array.isArray(d.trace.steps)){srElement('rule-flow-query-state').textContent='检查结果无效，请刷新或安装完整新版插件。';return;}
    if(RULE_RULE_REVISION && d.revision!==RULE_RULE_REVISION){invalidateRuleTrace('规则已变化，请刷新规则后重新检查。');srLoadRules();return;}
    RULE_TRACE=d;RULE_FLOW_CLIENT=d.source || '';RULE_FLOW_SELECTED=-1;
    if(SR_STATE.status){SR_STATE.status.proxy_mode=d.mode;SR_STATE.status.active_node=d.active_node;SR_STATE.status.running=d.running;SR_STATE.status.transparent_proxy=d.transparent_proxy;SR_STATE.status.cn_ipset_direct=d.cn_ipset_direct;}
    srElement('rule-flow-query-state').textContent='本次查询：'+d.destination+':'+d.port+' · '+(d.source ? '来源 '+d.source : '未指定设备，仅判断全局规则')+'。结果来自保存的配置，不代表既有连接或网页连通性。';
    renderRuleFlow();
  },function(e){
    if(!finish())return;
    var errors={route_trace_invalid:'输入无效，请核对网址、IP 和端口。',route_trace_fake_ip:'不能使用 Fake-IP，请填写域名或真实目的 IP。',route_trace_timeout:'DNS 查询超时，可填写真实目的 IP 后重试。',route_trace_config:'无法读取有效配置，请检查完整安装。',route_trace_busy:'已有分流检查正在进行，请稍后重试。'};
    var message=e && e.error_code==='route_trace_stale' ? '规则或模式已更新，已刷新规则，请重新检查。' : e && e.error==='Unknown action' ? '当前后端不支持分流检查，请通过软件中心安装完整新版插件。' : errors[e&&e.error_code] || '分流检查失败，请检查输入、DNS 或后端状态后重试。';
    srElement('rule-flow-query-state').textContent=message;renderRuleFlow();
    if(e && e.error_code==='route_trace_stale')srLoadRules();
  },20000);
}
function orderedFlowRules(rows) {
  var sorted=orderedRules(rows),compiled=SR_STATE.compiledTargets;
  if(!compiled)return sorted;
  return sorted.filter(function(r){return r.enabled===false || compiled[r.index]!=null;}).map(function(r){
    return r.enabled!==false && compiled[r.index]!==r.target ? Object.assign({},r,{target:compiled[r.index]}) : r;
  });
}
function byBindingPriority(a,b) {return (b.priority||0)-(a.priority||0);}
function ruleFlowModel(rows, mode, client, orderedRows, configuredOnly) {
  var deviceConfig=configuredOnly && client;
  if(mode==='global'||mode==='direct')return deviceConfig?[]:[{kind:'mode',title:mode==='global'?'全局代理':'全部直连',note:'跳过全部分流规则',targets:[mode==='global'?'proxy':'direct']}];
  var ordered=orderedRows||orderedFlowRules(rows), result=[];
  function group(kind,label,rules) {
    groupRulesForDisplay(rules).forEach(function(b){var members=b.indices.map(function(i){return rules[i];}),enabled=members.filter(function(r){return r.enabled!==false;}),first=Number(members[0].priority||0),last=Number(members[members.length-1].priority||0);
      result.push({kind:kind,bindingPriority:members[0].bindingPriority,title:b.name||'未分组',phaseLabel:label,note:'优先级 '+first+(first!==last?'～'+last:'')+' · 启用 '+enabled.length+'/'+members.length+(enabled.length?'':' · 已停用')+(kind.indexOf('device_')===0&&!client?' · 待指定设备':''),targets:(enabled.length?enabled:members).map(function(r){return r.target;}).filter(function(t,i,a){return a.indexOf(t)===i;}),count:members.length,enabledCount:enabled.length,disabled:!enabled.length,indices:members.map(function(r){return Number(r.index);}),rule:members.length===1?members[0]:null});
    });
  }
  function applies(source) {
    return client && source && (configuredOnly?sameDeviceSource(source,client):sourceContains(source,client));
  }
  var devices=ordered.filter(function(r){return applies(r.source);});
  group('device_rule','设备专属规则',devices.filter(function(r){return !isFinalRule(r);}));
  if(client) {
    var activeLinks=(SR_STATE.deviceBindings||[]).filter(function(b){return applies(b.source);})
      .sort(function(a,b){return byBindingPriority(a,b)||sourceNetwork(b.source).prefix-sourceNetwork(a.source).prefix;});
    var bestByGroup=new Map();
    activeLinks.forEach(function(b){if(!bestByGroup.has(b.group))bestByGroup.set(b.group,b);});
    var linked=[];
    ordered.forEach(function(r){
      if(r.source || isFinalRule(r))return;
      var link=bestByGroup.get((r.group||'').trim());
      if(link)linked.push(Object.assign({},r,{source:link.source,target:link.target||r.target,bindingPriority:link.priority||0}));
    });
    linked.sort(function(a,b){return b.bindingPriority-a.bindingPriority;});
    group('device_link','设备关联组',linked);
  }
  if(!deviceConfig){
    var shared=ordered.filter(function(r){return !r.source&&!isFinalRule(r);});
    if(SR_STATE.globalBindings){
      var bindings=new Map(SR_STATE.globalBindings.map(function(b){return [b.group,b];}));
      shared=shared.filter(function(r){return !(r.group||'').trim() || bindings.has(r.group.trim());}).map(function(r){
        var b=bindings.get((r.group||'').trim());
        return Object.assign({},r,{target:b&&b.target||r.target,bindingPriority:b&&b.priority||0});
      }).sort(function(a,b){return b.bindingPriority-a.bindingPriority;});
    }
    if(!hasDeviceAssignment(client))group('global_rule','全局自定义规则',shared);
    result.push({kind:'builtin',title:'内置国内 / 局域网',note:'核心内置国内域名、私有 IP 与 GeoIP 直连；独立于自定义规则，停用自定义 CN 规则不会关闭此层。',targets:['direct']});
  }
  function reachableFinals(rules) {
    var first=rules.findIndex(function(r){return r.enabled!==false;});
    return first<0?rules:rules.slice(0,first+1);
  }
  var deviceFinals=reachableFinals(devices.filter(isFinalRule));group('device_final','设备兜底',deviceFinals);
  if(client && hasDeviceAssignment(client)){
    if(!ordered.some(function(r){return r.source && r.enabled!==false && isFinalRule(r) && sourceContains(r.source,client);}))
      result.push({kind:'device_default',defaultFallback:true,title:'设备默认出口',note:'设备规则未命中时使用代理',targets:['proxy']});
    return result;
  }
  if(deviceConfig)return result;
  var finals=reachableFinals(ordered.filter(function(r){return !r.source&&isFinalRule(r);}));group('global_final','全局兜底',finals);
  if(!finals.some(function(r){return r.enabled!==false;}))result.push({kind:'global_final',defaultFallback:true,title:'全局兜底',note:'没有启用的全局最终规则，未命中时代理',targets:['proxy']});
  return result;
}
function selectRuleFlow(index) {
  var row=ruleFlowModel(SR_STATE.ruleRows||[],RULE_SNAPSHOT_MODE||'rule',RULE_FLOW_CLIENT)[index];
  if(row && row.indices){var member=(SR_STATE.ruleRows||[]).find(function(r){return Number(r.index)===Number(row.indices[0]);});if(member && !changeRuleGroup((member.group||'').trim()))return false;}
  RULE_FLOW_SELECTED=index;renderRuleFlow();var button=srElement('rule-flow-step-'+index);if(button)button.focus();return true;
}
function toggleRuleFlow() {
  var host=srElement('rule-flow'),button=srElement('rule-flow-toggle');host.hidden=!host.hidden;
  button.textContent=host.hidden?'展开概览':'收起概览';button.setAttribute('aria-expanded',String(!host.hidden));
}
function showFlowRuleList(kind, index) {
  if(typeof index==='number') {if(selectRuleFlow(index)===false)return;}
  else if(!changeRuleGroup(RULE_SELECTED_GROUP))return;
  srElement('rule-workspace').scrollIntoView({behavior:'smooth',block:'start'});
}
function ruleSourceLabel(rule) {
  return rule.source || ((rule.group||'').trim() && !isFinalRule(rule)?'由组绑定决定':'全局设备');
}
function flowRuleKind(rule){return rule.source ? (isFinalRule(rule)?'device_final':'device_rule') : (isFinalRule(rule)?'global_final':'global_rule');}
function flowMatchedRule(edit) {
  if(rulePending||RULE_READ_PENDING||RULE_FLOW_ERROR)return;
  if(!RULE_TRACE || RULE_TRACE.trace.matched_rule==null)return;
  var index=Number(RULE_TRACE.trace.matched_rule), rule=(SR_STATE.ruleRows||[]).filter(function(r){return Number(r.index)===index;})[0], expected=RULE_TRACE.trace.matched_snapshot;
  if(!rule || !expected || RULE_TRACE.revision!==RULE_RULE_REVISION || ['source','name','group','rule_type','pattern','target','priority','enabled'].some(function(k){return rule[k]!==expected[k];})){
    invalidateRuleTrace('规则已变化，请重新检查后编辑。');srLoadRules();return;
  }
  if(edit)editRule(index);else inspectRule(index);
}
function flowTraceHtml(data) {
  if(!data)return '';
  var t=data.trace, row=t.matched_rule==null?null:(SR_STATE.ruleRows||[]).filter(function(r){return Number(r.index)===Number(t.matched_rule);})[0];
  var title=(t.definitive?'判定：':'待验证：')+(RULE_TARGET_LABEL[t.action]||t.action);
  var reason=data.mode!=='rule' ? '当前模式跳过分流规则。' : row ? '命中配置 #'+(Number(t.matched_rule)+1)+' · '+(row.name||row.group||RULE_TYPE_LABEL[row.rule_type]||row.rule_type)+' · '+row.pattern+' · 优先级 '+Number(row.priority||0) : t.matched_builtin ? '命中核心内置规则。' : hasDeviceAssignment(data.source)?'使用设备默认出口。':'使用全局兜底。';
  var dns={literal:'目的地址为 IP',provided:'使用填写的真实目的 IP',resolved:'按需 DNS 解析成功',not_needed:'命中前无需 DNS 解析',failed:'DNS 解析失败，IP 规则尚未验证',skipped:'未查询 DNS，IP 规则尚未验证'};
  var warnings=[];
  if(!data.running)warnings.push('代理已停止：以下仅为保存配置的判定。');
  if(!data.transparent_proxy)warnings.push('透明接管未开启：LAN 设备需手动设置路由器 HTTP/SOCKS 代理才能使用这些规则。');
  if(data.lan_degraded)warnings.push('LAN 当前处于直连保护，实际流量可能绕过代理。');
  if(data.cn_ipset_direct)warnings.push('已配置内核国内 IP 绕过，部分 LAN 流量可能先于核心规则直连。');
  if(!t.definitive)warnings.push('尚不能确认最终路径，请允许 DNS 查询或填写设备实际获得的真实目的 IP 后重试。');
  if(t.action==='proxy' && data.active_node==null)warnings.push('尚未选择可用代理节点；规则指向代理不代表能建立连接。');
  if(t.action.indexOf('group:')===0){reason+=(data.group_type==='fallback'?' · 首选成员（TCP 失败后尝试备用）：':' · 出口成员：')+(data.node_name||'不可用');if(!data.node_name)warnings.push('分组当前成员不可用；实际连接将报错，请重新选择成员。');}
  if(t.action.indexOf('node:')===0){reason+=' · 指定节点：'+(data.node_name||'不可用');if(!data.node_name)warnings.push('指定节点不存在，连接会失败，不会改走全局节点。');}
  var labels={source_mismatch:'来源不符 · 跳过',needs_ip:'缺少 IP · 待验证',not_matched:'未命中',matched:t.definitive?'命中 · 此处结束':'暂定匹配 · 前序 IP 条件待验证'};
  var checks=t.steps.map(function(s){var r=s.config_index==null?null:(SR_STATE.ruleRows||[]).filter(function(r){return Number(r.index)===Number(s.config_index);})[0];
    var text=s.rule_type==='builtin'?'内置规则 · 已检查 '+s.checked+' 项':s.config_index!=null?'配置 #'+(Number(s.config_index)+1)+' · '+(r ? (r.name||r.group||r.pattern||'兜底') : s.pattern):'内置 '+(RULE_TYPE_LABEL[s.rule_type]||s.rule_type)+' · '+(s.pattern||'');
    var condition=s.rule_type==='builtin'?'内置国内 / 局域网条件汇总':'来源 '+(s.source||'全局')+' · '+(RULE_TYPE_LABEL[s.rule_type]||s.rule_type)+' '+(s.pattern||'')+' → '+(RULE_TARGET_LABEL[s.action]||s.action||'');
    return '<li class="'+(s.outcome==='matched'?'matched':'')+'">'+srEsc(text)+' <span class="badge">'+srEsc(labels[s.outcome]||s.outcome)+'</span><div class="hint">'+srEsc(condition)+'</div></li>';
  }).join('');
  var actions=row?'<button class="btn btn-sm" type="button" onclick="flowMatchedRule(false)">查看命中规则</button><button class="btn btn-accent btn-sm" type="button" onclick="flowMatchedRule(true)">编辑命中规则</button>':'';
  if(t.action==='proxy'||t.action.indexOf('group:')===0||t.action.indexOf('node:')===0)actions+='<button class="btn btn-sm" type="button" onclick="srShowPage(\'nodes\')">查看出口节点</button>';
  return '<section class="flow-trace-result" aria-live="polite"><h4>'+srEsc(title)+'</h4><p class="hint">'+srEsc(reason)+'</p><p class="hint">'+srEsc(dns[t.dns]||t.dns)+(t.resolved_ip?' · '+srEsc(t.resolved_ip):'')+'</p>'+warnings.map(function(w){return '<p class="flow-warning">'+srEsc(w)+'</p>';}).join('')+'<div class="toolbar" style="margin-top:10px">'+actions+'</div>'+(checks?'<details open><summary>判定过程 · 已检查 '+Number(t.checked_rules)+' 项</summary><ol class="flow-checks">'+checks+'</ol>'+(t.omitted_steps?'<p class="hint">中间省略 '+Number(t.omitted_steps)+' 项；实际匹配未截断，命中规则始终保留。</p>':'')+'</details>':'')+'<p class="hint">使用核心相同的规则顺序与匹配器。已有连接缓存、TUN DNS 映射提示及内核接管状态可能影响实际访问；本检查不验证网页是否可达，不保存查询历史。</p></section>';
}
function renderRuleFlow() {
  var host=srElement('rule-flow');if(!host)return;
  if(RULE_TRACE_PENDING && RULE_TRACE_CONTEXT!==ruleTraceContext()){invalidateRuleTrace('服务、分流模式或出口已变化，请重新检查。');return;}
  var st=SR_STATE.status||{},mode=String(RULE_SNAPSHOT_MODE||st.proxy_mode||'rule').toLowerCase(),known=SR_STATE.statusKnown,running=!!st.running;
  var active=(Number(st.node_count)===0)?null:(st.active_node==null?null:Number(st.active_node));
  if(RULE_TRACE && (RULE_TRACE.mode!==mode || RULE_TRACE.active_node!==active || RULE_TRACE.running!==running || RULE_TRACE.transparent_proxy!==!!st.transparent_proxy || RULE_TRACE.cn_ipset_direct!==!!st.cn_ipset_direct)){
    RULE_TRACE=null;RULE_TRACE_GENERATION++;srElement('rule-flow-query-state').textContent='服务、分流模式或出口已变化，请重新检查。';
  }
  srElement('rule-flow-state').textContent=(!known?'状态未知':running?'服务运行中':'服务已停止')+' · '+(mode==='global'?'全局模式':mode==='direct'?'直连模式':'规则模式');
  if(!Array.isArray(SR_STATE.ruleRows)){host._flowHtml=null;host.innerHTML='<p class="hint">'+(RULE_FLOW_ERROR?'规则加载失败，请刷新后重试。':'正在加载规则…')+'</p>';return;}
  var rows=ruleFlowModel(SR_STATE.ruleRows,mode,RULE_FLOW_CLIENT),t=RULE_TRACE&&RULE_TRACE.trace,winner=-1;
  if(t){
    var matched=t.steps.find(function(step){return step.outcome==='matched';}),kind;
    if(mode!=='rule')kind='mode';
    else if(t.matched_rule!=null){
      var original=SR_STATE.ruleRows.find(function(r){return Number(r.index)===Number(t.matched_rule);});
      if(original)kind=matched && matched.source && !original.source?'device_link':flowRuleKind(original);
    }else kind=t.matched_builtin && matched && matched.rule_type!=='final'?'builtin':hasDeviceAssignment(RULE_FLOW_CLIENT)?'device_default':'global_final';
    winner=rows.findIndex(function(r){
      return r.kind===kind && !r.disabled && (t.matched_rule!=null ? r.indices&&r.indices.includes(Number(t.matched_rule)) : kind!=='global_final'||r.defaultFallback);
    });
  }
  var all=SR_STATE.ruleRows,enabled=all.filter(function(r){return r.enabled!==false;}).length;
  srElement('rule-flow-summary').textContent='与下方同一配置：共 '+all.length+' 条 · 启用 '+enabled+' 条 · 停用 '+(all.length-enabled)+' 条。图中不受列表搜索或分页影响。'+(rulePending?' 正在保存/应用并核对结果…':RULE_READ_PENDING?' 正在读取最新状态…':RULE_FLOW_ERROR?' 当前为上次快照，请刷新。':'');
  if(RULE_FLOW_SELECTED<0 && winner>=0)RULE_FLOW_SELECTED=winner;
  RULE_FLOW_SELECTED=Math.max(0,Math.min(RULE_FLOW_SELECTED,rows.length-1));
  var html=sharedFlowGraph(all,mode,rows,winner)+flowTraceHtml(RULE_TRACE);
  if(RULE_FLOW_ERROR)html+='<p class="flow-warning">规则刷新失败，显示的是上次快照。</p>';
  if(host._flowHtml!==html){host.innerHTML=html;host._flowHtml=html;}

}

function refreshRuleGroupList(rules) {
  var dl = srElement('rule-group-list');
  if (!dl) return;
  var seen = Object.create(null);
  var opts = [];
  (rules || []).forEach(function (r) {
    var g = (r.group || '').trim();
    if (g && !seen[g]) { seen[g] = 1; opts.push('<option value="' + srEsc(g) + '">'); }
  });
  dl.innerHTML = opts.join('');
}
function ruleRowHtml(r, position, ordered) {
  var dim = r.enabled === false ? ' style="opacity:.6"' : '';
  var all = ordered || orderedRules(SR_STATE.ruleRows || []), pos = position === undefined ? all.indexOf(r) : position;
  function edge(dir) { var next = all[pos + dir]; return !next || rulePhase(next) !== rulePhase(r) ? ' disabled' : ''; }
  return '<div class="row rule-row"' + dim + '><div class="rule-main">' +
    '<span class="badge acc">' + srEsc(RULE_TYPE_LABEL[r.rule_type] || r.rule_type) + '</span> ' +
    '<span class="badge">' + srEsc(RULE_TARGET_LABEL[r.target] || r.target) + '</span>' +
    '<div class="rule-pattern">' + srEsc(isFinalRule(r) ? '未命中的其余流量' : r.pattern) + '</div>' +
    '<span class="rule-meta">' + (r.enabled === false ? '已停用' : '') + '</span></div>' +
    '<div class="rule-actions"><button class="btn btn-sm" onclick="editRule(' + Number(r.index) + ')">编辑</button>' +
    '<details><summary>更多</summary><div class="toolbar"><button class="btn btn-sm" onclick="inspectRule(' + Number(r.index) + ')">详情</button>' +
    '<button class="btn btn-sm" onclick="moveRule(' + Number(r.index) + ',-1)" aria-label="上移规则"' + edge(-1) + '>↑</button>' +
    '<button class="btn btn-sm" onclick="moveRule(' + Number(r.index) + ',1)" aria-label="下移规则"' + edge(1) + '>↓</button>' +
    '<button class="btn btn-danger btn-sm" onclick="delRule(' + Number(r.index) + ')">删除</button></div></details></div></div>';
}
var rulePending = false;
function renderRuleBusy() {srElement('device-form').disabled=ruleWorkspaceLocked();renderRulePreset();if(Array.isArray(SR_STATE.ruleRows))renderRules(false);}
function ruleApi(action, data, cb, errCb) {
  if (action === 'get_rules' || action === 'export_rules') return srApi(action, data, cb, errCb);
  if (rulePending||RULE_READ_PENDING||RULE_FLOW_ERROR) { srShowMsg('请等待规则保存、应用及刷新完成；读取失败时请先刷新。','err'); return; }
  rulePending = true;
  RULE_LOAD_GENERATION++;
  var renaming=action==='save_device_alias';
  if(!renaming)invalidateRuleTrace('规则正在更新，完成后请重新检查。');
  srElement('rule-operation-state').textContent=renaming?'正在保存设备名称…':'正在保存并应用规则，完成后将核对最新状态；请勿重复点击。';
  renderRuleBusy();
  function reconcile(d,failed) {
    srLoadRules(function(fresh){
      rulePending=false;renderRuleBusy();
      var message=failed?'请求未完成，已尝试读取保存结果；应用状态未确认，请结合诊断检查，勿重复提交。':d.ok===false?(d.msg||d.error_message||'规则更新失败；已重新读取保存状态。'):d.saved&&d.applied===false?'规则已保存；代理当前停止，下次启动后生效。':renaming?'设备名称已保存。':'规则已保存并完成状态核对。';
      if(!fresh)message+=' 最新状态读取失败，当前为上次快照，请先刷新。';
      srElement('rule-operation-state').textContent=message;
      if(fresh&&!RULE_TRACE&&!RULE_TRACE_PENDING)srElement('rule-flow-query-state').textContent='已核对保存的配置，输入网站可重新检查规则判定。';
      if(failed){srShowMsg(message,'err');if(errCb)errCb(d);}
      else if(cb)cb(d);
    });
  }
  srApi(action,data,function(d){reconcile(d||{ok:false,msg:'接口结果无效'},false);},function(e){reconcile(e||{},!(e&&e.saved===true&&e.applied===false));},60000);
}
function srLoadRules(after) {
  if(rulePending&&typeof after!=='function')return;
  var generation=++RULE_LOAD_GENERATION;RULE_READ_PENDING=true;renderRuleBusy();
  function fail(message){
    if(generation!==RULE_LOAD_GENERATION)return;
    RULE_READ_PENDING=false;RULE_FLOW_ERROR=true;invalidateRuleTrace('规则读取失败，图与列表均为上次快照。');renderRuleBusy();
    srElement('rule-operation-state').textContent=message;
    if(typeof after==='function')after(false);
  }
  ruleApi('get_rules', null, function (d) {
    if(generation!==RULE_LOAD_GENERATION)return;
    if (!d || d.ok === false || !Array.isArray(d.rules)) {fail('规则读取失败，请刷新后重试。');return;}
    var rules = d.rules;
    SR_STATE.devices = d.devices || [];
    SR_STATE.globalBindings = d.global_rule_groups || null;
    SR_STATE.deviceBindings = (d.device_rule_groups || []).concat(SR_STATE.globalBindings || []);
    SR_STATE.compiledTargets = Array.isArray(d.compiled_targets)?d.compiled_targets:null;
    SR_STATE.ruleRows = rules; RULE_FLOW_ERROR = false;RULE_READ_PENDING=false;
    RULE_SNAPSHOT_MODE=typeof d.proxy_mode==='string'?d.proxy_mode:(SR_STATE.status||{}).proxy_mode||'rule';
    var revision=d.revision||null, changed=RULE_RULE_REVISION!==revision;
    RULE_RULE_REVISION=revision;
    if(changed&&RULE_FLOW_FILTER&&RULE_FLOW_FILTER.indices)RULE_FLOW_FILTER=null;
    setRuleNodes(d.nodes || []);
    setProxyGroupTargets(d.proxy_groups || []);
    syncFlowDevicePicker();
    renderDevicePolicies();
    loadRulePresets(d.presets);
    if(changed && (RULE_TRACE || RULE_TRACE_PENDING))invalidateRuleTrace('规则或模式已更新，请重新检查。');
    refreshRuleGroupList(rules);
    renderRules(false);
    if(typeof after==='function')after(true);
    else srElement('rule-operation-state').textContent='';
  }, function () {fail('规则读取失败，请刷新后重试。');});
}
var RULE_PAGE = 0, RULE_PAGE_SIZE = 50;
function changeRulePage(dir) { RULE_PAGE += dir; renderRules(false); }
function renderRules(resetPage) {
  syncRuleWorkspace();renderRuleFlow();
  if(resetPage!==false)RULE_PAGE=0;
  var el=srElement('rule-list'),members=ruleGroupMembers(RULE_SELECTED_GROUP),sorted=orderedRules(SR_STATE.ruleRows||[]),positions=Object.create(null);
  sorted.forEach(function(r,i){positions[r.index]=i;});
  var query=srElement('rule-search').value.trim().toLowerCase(),target=srElement('rule-filter-target').value,state=srElement('rule-filter-state').value,scope=srElement('rule-filter-scope').value;
  var rules=sorted.filter(function(r){return (r.group||'').trim()===RULE_SELECTED_GROUP &&
    (!scope || (scope==='global' ? !r.source : !!r.source)) &&
    (!query || query===(RULE_SELECTED_GROUP||'').toLowerCase() || [r.pattern,r.source,r.name,RULE_TYPE_LABEL[r.rule_type]].join(' ').toLowerCase().indexOf(query)>=0) &&
    (!target || r.target===target) && (!state || (state==='disabled'?r.enabled===false:r.enabled!==false));});
  srElement('rule-count').hidden=!(query || target || scope || state || rulePending || RULE_READ_PENDING || RULE_FLOW_ERROR);
  srElement('rule-count').textContent='本组共 '+members.length+' 条 · 当前显示 '+rules.length+' 条'+(rulePending?' · 更新核对中':RULE_READ_PENDING?' · 刷新中':RULE_FLOW_ERROR?' · 上次快照':'');
  var pages=Math.max(1,Math.ceil(rules.length/RULE_PAGE_SIZE));RULE_PAGE=Math.max(0,Math.min(RULE_PAGE,pages-1));
  srElement('rule-pager').hidden=rules.length<=RULE_PAGE_SIZE;
  srElement('rule-page-label').textContent='第 '+(RULE_PAGE+1)+' / '+pages+' 页 · 每页最多 '+RULE_PAGE_SIZE+' 条';
  srElement('rule-page-prev').disabled=RULE_PAGE===0;srElement('rule-page-next').disabled=RULE_PAGE===pages-1;
  rules=rules.slice(RULE_PAGE*RULE_PAGE_SIZE,(RULE_PAGE+1)*RULE_PAGE_SIZE);
  var phase=-1,labels=['设备目的规则','组内或未分组目的规则','设备兜底（内置国内直连之后）','全局兜底'];
  el.innerHTML=rules.length?rules.map(function(r){var next=rulePhase(r),head=next!==phase?'<div class="rule-phase">'+labels[next]+'</div>':'';phase=next;return head+ruleRowHtml(r,positions[r.index],sorted);}).join(''):'<div class="hint">'+(members.length?'没有匹配的规则，请调整筛选条件。':'本组还没有规则，点击「添加规则」开始。')+'</div>';
  if(ruleWorkspaceLocked()){if(el.querySelectorAll)Array.prototype.forEach.call(el.querySelectorAll('.rule-actions button'),function(button){button.disabled=true;});}
}

function proxyLikeTarget(target) {
  return target==='proxy' || (typeof target==='string' && (target.indexOf('group:')===0 || target.indexOf('node:')===0));
}
function groupOutletSelect(enc, members) {
  var nodes=SR_RULE_NODES||[];
  var proxyLike=members.filter(function(r){return proxyLikeTarget(r.target);});
  var shared=proxyLike.length && proxyLike.every(function(r){return r.target===proxyLike[0].target;}) ? proxyLike[0].target : '';
  var options=[{value:'proxy',label:'全局节点'}].concat(nodes.map(function(n){return {value:'node:'+n.id,label:'节点 · '+n.name};})).concat((SR_PROXY_GROUPS||[]).map(function(g){return {value:'group:'+g.name,label:'分组 · '+g.name};}));
  var known=options.some(function(o){return o.value===shared;});
  var blank=proxyLike.length ? (shared&&known?'':'<option value="">出口不一致</option>') : '<option value="">选择出口</option>';
  if(shared && !known)options.push({value:shared,label:'已失效'});
  return '<select id="sr-rule-group-outlet-select" aria-label="分组出口"'+(ruleWorkspaceLocked() || RULE_ADD_BASELINE!==null || RULE_ROW_BASELINE!==null || !proxyLike.length?' disabled':'')+' onchange="setRuleGroupOutlet(\''+enc+'\',this.value)">'+blank+options.map(function(o){return '<option value="'+srEsc(o.value)+'"'+(o.value===shared?' selected':'')+'>'+srEsc(o.label)+'</option>';}).join('')+'</select>';
}
function useRuleGroup(enc) {
  changeRuleGroup(decodeURIComponent(enc));
}
function toggleRuleGroup(enc) {
  var key = decodeURIComponent(enc);
  RULE_COLLAPSED[key] = !RULE_COLLAPSED[key];
  renderRules(false);
}
function setRuleGroupOutlet(enc, target) {
  if(!target){renderRules(false);return;}
  var group=decodeURIComponent(enc),members=(SR_STATE.ruleRows||[]).filter(function(r){return (r.group||'').trim()===group.trim();}).map(function(r){var expected=Object.assign({},r);delete expected.index;return expected;});
  ruleApi('set_rule_group_target', { group: group, target: target, expected_rules: members }, function (d) { toastResp(d); });
}
function setRuleGroupEnabled(enc, enabled) {
  var group=decodeURIComponent(enc),members=(SR_STATE.ruleRows||[]).filter(function(r){return (r.group||'').trim()===group.trim();}).map(function(r){var expected=Object.assign({},r);delete expected.index;return expected;});
  ruleApi('set_rule_group_enabled', { group: group, enabled: !!enabled, expected_rules:members }, function (d) {
    toastResp(d);
  });
}
function deleteRuleGroup(enc) {
  var name = decodeURIComponent(enc) || '未分组';
  if (!confirm('删除分组「' + name + '」内的全部规则及设备对该组的关联？')) return;
  ruleApi('delete_rule_group', { group: decodeURIComponent(enc) }, function (d) {
    toastResp(d);
  });
}
function inspectRule(index) {
  var rule = (SR_STATE.ruleRows || []).filter(function (r) { return Number(r.index) === Number(index); })[0];
  if (!rule) return;
  srElement('rule-detail-text').textContent = '来源：' + ruleSourceLabel(rule) + '\n类型：' + (RULE_TYPE_LABEL[rule.rule_type] || rule.rule_type) + '\n匹配：' + rule.pattern + '\n处理：' + (RULE_TARGET_LABEL[rule.target] || rule.target) + '\n优先级：' + Number(rule.priority || 0) + '\n分组：' + (rule.group || '未分组') + '\n状态：' + (rule.enabled === false ? '已停用' : '已启用');
  srElement('rule-dialog').showModal();
}
var RULE_EDIT_ROW = null;
function editRule(index) {
  var r = (SR_STATE.ruleRows || []).filter(function(r) { return Number(r.index) === Number(index); })[0];
  if (!r || ruleWorkspaceLocked() || !discardRuleDraft()) return;
  resetRuleEditor();RULE_SELECTED_GROUP=(r.group||'').trim();
  RULE_EDIT_ROW = JSON.parse(JSON.stringify(r));
  ['source','group','pattern','target','priority'].forEach(function(k) { srElement('edit-rule-' + k).value = r[k] == null ? '' : r[k]; });
  srElement('edit-rule-type').value = r.rule_type === 'match' ? 'final' : r.rule_type === 'port' ? 'dst-port' : r.rule_type;
  srElement('edit-rule-enabled').checked = r.enabled !== false;
  editRuleTypeChanged();srElement('rule-edit-panel').hidden=false;RULE_ROW_BASELINE=ruleRowSnapshot();renderRules(false);srElement('rule-edit-panel').scrollIntoView({block:'center',behavior:'smooth'});
}
function editRuleTypeChanged() {
  var t = srElement('edit-rule-type').value;
  srElement('edit-rule-pattern').disabled = t === 'final';
  srElement('edit-rule-pattern').placeholder = RULE_PATTERN_HINT[t] || '';
}
function saveRuleEdit() {
  if (!RULE_EDIT_ROW || ruleWorkspaceLocked()) return;
  var priority = readRulePriority('edit-rule-priority'); if (priority === false || priority === null) { srShowMsg('请填写有效的整数优先级。','err'); return; }
  var draft=ruleRowSnapshot(), original = RULE_EDIT_ROW, expected = Object.assign({}, original); delete expected.index; expected.source=original.source || '';
  var data = {index:original.index, expected:expected, source:srElement('edit-rule-source').value.trim(), rule_type:srElement('edit-rule-type').value,
    pattern:srElement('edit-rule-pattern').value.trim(), group:srElement('edit-rule-group').value.trim(),
    priority:priority, target:srElement('edit-rule-target').value, enabled:srElement('edit-rule-enabled').checked};
  if (data.rule_type === 'final') data.pattern = '*';
  if (!data.pattern) { srShowMsg('请输入匹配内容。','err'); return; }
  srElement('rule-edit-save').disabled = true;
  ruleApi('update_rule',data,function(d) {
    srElement('rule-edit-save').disabled = false; toastResp(d);
    if (d.ok !== false) { if (RULE_EDIT_ROW === original && draft===ruleRowSnapshot()) { resetRuleEditor();RULE_SELECTED_GROUP=data.group;renderRules(false); } }
  },function() { srElement('rule-edit-save').disabled = false; });
}
function cancelRuleEditor() {if(discardRuleDraft()){resetRuleEditor();renderRules(false);}}
function resetRuleEditor() {
  RULE_EDIT_GROUP=null;RULE_EDIT_MEMBERS=null;RULE_EDIT_ROW=null;RULE_ADD_BASELINE=null;RULE_ROW_BASELINE=null;RULE_WORKSPACE_MODE='list';
  RULE_IMPORT_MODE=false;RULE_IMPORT_PENDING=false;RULE_IMPORT_GENERATION++;RULE_PRESET_GROUP=null;renderRuleAddMethod('manual');
  srElement('rule-import-panel').hidden=true;srElement('rule-import-url').value='';srElement('rule-import-state').textContent='';
  srElement('rule-add-panel').hidden=true;srElement('rule-edit-panel').hidden=true;
  srElement('rule-priority').value='0';srElement('rule-source').value='';srElement('rule-type').value='domain-suffix';
  srElement('rule-entry-mode').value='single';srElement('rule-entry-mode').disabled=false;
  srElement('rule-group').value=RULE_SELECTED_GROUP || '';srElement('rule-batch').value='';srElement('rule-pattern').value='';ruleTypeChanged();
}
var rulePreviewSnapshot = null;

function ruleEditorSnapshot() { return JSON.stringify(['rule-source','rule-entry-mode','rule-group','rule-type','rule-target','rule-dup','rule-priority','rule-batch','rule-pattern','rule-import-url'].map(function(id) { return srElement(id).value; }).concat([RULE_EDIT_GROUP,RULE_IMPORT_MODE,RULE_PRESET_GROUP,RULE_PRESET_GROUP===null?'':srElement('rule-preset').value,RULE_PRESET_GROUP===null?'':srElement('preset-route-target').value])); }
function readRulePriority(id) {
  var text=String(srElement(id).value).trim(); if (!text) return null;
  if (!/^-?\d+$/.test(text)) return false;
  var value=Number(text); return Number.isInteger(value) && value>=-2147483648 && value<=2147483647 ? value : false;
}
function previewRuleChanges() {
  if (readRulePriority('rule-priority') === false) {srShowMsg('优先级必须是有效的整数。','err');return;}

  var bulk = RULE_EDIT_GROUP !== null || srElement('rule-entry-mode').value === 'bulk';
  var text = (bulk ? srElement('rule-batch').value : srElement('rule-pattern').value).trim();
  if (!text && (bulk || srElement('rule-type').value !== 'final')) { srShowMsg('请先填写或粘贴规则。','err'); return; }
  rulePreviewSnapshot = ruleEditorSnapshot();
  srElement('rule-preview-title').textContent = RULE_EDIT_GROUP !== null ? '确认替换整个分组' : '确认添加规则';
  srElement('rule-preview-apply').textContent = RULE_EDIT_GROUP !== null ? '保存整组修改' : RULE_IMPORT_MODE ? '添加这 '+text.split(/\r?\n/).filter(function(line){return line.trim() && !/^\s*[#;]/.test(line);}).length+' 条规则' : '添加到「'+(RULE_SELECTED_GROUP||'未分组')+'」';
  srElement('rule-preview-text').textContent = text || 'MATCH,' + srElement('rule-target').value.toUpperCase();
  srElement('rule-preview-summary').textContent = '分组：' + (srElement('rule-group').value.trim() || '未分组') + (RULE_EDIT_GROUP !== null ? '。这会替换整个分组，并将其集中到首次出现的位置；未修改规则的启用状态和优先级会保留。' : '。类型：' + (RULE_IMPORT_MODE ? '链接规则，以每行类型与出口为准' : RULE_TYPE_LABEL[srElement('rule-type').value] || '混合') + '；默认处理：' + (RULE_TARGET_LABEL[srElement('rule-target').value] || srElement('rule-target').value) + (RULE_IMPORT_MODE ? '。' : '。现有规则按所选重复处理方式保留或替换。'));
  srElement('rule-preview-summary').textContent += ' 优先级：'+(readRulePriority('rule-priority') === null ? (RULE_EDIT_GROUP !== null ? '保留原值，新规则沿用分组优先级' : '保留原值，新规则为 0') : readRulePriority('rule-priority'))+'（数字越大越先匹配）。';
  srElement('rule-preview-summary').textContent += ' 来源：'+(RULE_EDIT_GROUP !== null ? '保留每行 SOURCE 或原分组来源' : srElement('rule-source').value.trim() || (RULE_IMPORT_MODE?'由组绑定决定':'按组绑定、规则来源或兜底设置生效'))+'。';
  if (RULE_EDIT_GROUP !== null) srElement('rule-preview-summary').textContent += ' 未填写 ACTION 的行默认处理：'+(RULE_TARGET_LABEL[srElement('rule-target').value] || srElement('rule-target').value)+'。';
  if(RULE_IMPORT_MODE)srElement('rule-preview-summary').textContent+=' 本次追加到当前组；当前组内同类型、匹配内容和来源的已有规则跳过，不替换现有规则。';
  srElement('rule-preview-dialog').showModal();
}
function applyRulePreview() {
  if (rulePreviewSnapshot !== ruleEditorSnapshot()) { srShowMsg('编辑内容已变化，请重新预览。','err'); srElement('rule-preview-dialog').close(); return; }
  srElement('rule-preview-dialog').close(); addRule();
}
var RULE_EDIT_GROUP = null;
var RULE_EDIT_MEMBERS = null;
function editRuleGroup(enc) {
  var key = decodeURIComponent(enc);
  var members = (SR_STATE.ruleRows || []).filter(function (r) {
    return (r.group || '').trim() === key;
  });
  if (!members.length || ruleWorkspaceLocked() || !discardRuleDraft()) return;
  resetRuleEditor();RULE_SELECTED_GROUP=key;RULE_WORKSPACE_MODE='bulk';srElement('rule-add-panel').hidden=false;
  srElement('rule-priority').value = '';
  RULE_EDIT_GROUP = key;renderRuleAddMethod('manual');
  RULE_EDIT_MEMBERS = members.map(function(r) { var copy = Object.assign({},r); delete copy.index; return copy; });
  srElement('rule-entry-mode').value = 'bulk';
  srElement('rule-editor-state').textContent = '批量修改「' + (key || '未分组')+'」的规则';
  srElement('rule-edit-cancel').hidden = false;
  srElement('rule-editor-state').scrollIntoView({block:'center',behavior:'smooth'});
  srElement('rule-group').value = key;
  var typ = members[0].rule_type === 'match' ? 'final' : members[0].rule_type === 'port' ? 'dst-port' : members[0].rule_type;
  srElement('rule-type').value = typ;
  srElement('rule-target').value = members[0].target;
  // A removed node/group may be absent from the native select's options.
  // Only the default changes; explicit actions in the text remain untouched.
  if (!srElement('rule-target').value || !Object.prototype.hasOwnProperty.call(RULE_TARGET_LABEL, members[0].target)) srElement('rule-target').value = 'proxy';
  function clashLine(r) {
    var T = {
      'domain': 'DOMAIN', 'domain-suffix': 'DOMAIN-SUFFIX', 'domain-keyword': 'DOMAIN-KEYWORD',
      'ip-cidr': 'IP-CIDR', 'geoip': 'GEOIP', 'dst-port': 'DST-PORT', 'port': 'DST-PORT',
      'match': 'MATCH', 'final': 'MATCH'
    };
    var act = /^(group:|node:)/.test(r.target || 'proxy')?r.target:(r.target || 'proxy').toUpperCase();
    var line = (r.rule_type === 'match' || r.rule_type === 'final') ? 'MATCH,'+act : (T[r.rule_type] || r.rule_type.toUpperCase())+','+r.pattern+','+act;
    return r.source ? 'SOURCE,'+r.source+','+line : line;
  }
  var text = members.map(clashLine).join('\n');
  ruleTypeChanged();
  srElement('rule-bulk-wrap').style.display = '';
  srElement('rule-single-wrap').style.display = 'none';
  srElement('rule-batch').value = text;
  srElement('rule-pattern').value = '';
  RULE_ADD_BASELINE=ruleEditorSnapshot();renderRules(false);
}
function addRule() {
  var priority = readRulePriority('rule-priority');
  if (priority === false) {srShowMsg('优先级必须是有效的整数。','err');return;}
  function withPriority(data) {if(priority !== null)data.priority=priority;if(RULE_EDIT_GROUP === null && srElement('rule-source').value.trim())data.source=srElement('rule-source').value.trim();return data;}
  var t = srElement('rule-type').value;
  var grp = srElement('rule-group').value.trim();
  var g = srElement('rule-target').value;
  var replace = srElement('rule-dup').value === 'replace';
  if (RULE_EDIT_GROUP !== null) {
    if (!grp && RULE_EDIT_GROUP) { srShowMsg('请输入分组名称', 'err'); return; }
    var editText = srElement('rule-batch').value.trim()
      || srElement('rule-pattern').value.trim();
    if (t !== 'final' && !editText) { srShowMsg('请先粘贴该分组的规则', 'err'); return; }
    var oldGroup = RULE_EDIT_GROUP, draft = ruleEditorSnapshot();
    ruleApi('replace_rule_group', withPriority({
      group: oldGroup, new_group: grp, rule_type: t, target: g, text: editText, expected_rules: RULE_EDIT_MEMBERS
    }), function (d) {
      toastResp(d);
      if (d.ok !== false) {
        if (draft === ruleEditorSnapshot()) {resetRuleEditor();renderRules(false);}
      }
    });
    return;
  }
  if (RULE_SELECTED_GROUP === null) { srShowMsg('请先选择或新建分组。', 'err'); return; }
  if (srElement('rule-entry-mode').value === 'bulk') {
    var text = srElement('rule-batch').value, batchDraft=ruleEditorSnapshot();
    if (!text.trim()) { srShowMsg('请先粘贴至少一条规则', 'err'); return; }
    ruleApi('batch_add_rules', withPriority({ text: text, target: g, group: grp, rule_type: t, replace: RULE_IMPORT_MODE ? false : replace, group_only: RULE_IMPORT_MODE }), function (d) {
      toastResp(d);
      if (d.ok !== false) {
        if (ruleEditorSnapshot() === batchDraft) {resetRuleEditor();renderRules(false);}
      }
    });
    return;
  }
  var p = srElement('rule-pattern').value.trim(), singleDraft=ruleEditorSnapshot();
  if (t !== 'final' && !p) { srShowMsg('请输入匹配内容', 'err'); return; }
  ruleApi('add_rule', { group: grp, source:srElement('rule-source').value.trim(), rule_type: t, pattern: p, target: g, priority: priority === null ? 0 : priority }, function (d) {
    toastResp(d);
    if (d.ok !== false) {
      if (ruleEditorSnapshot()===singleDraft) {resetRuleEditor();renderRules();srShowMsg('已向「'+(grp||'未分组')+'」添加规则，已启用。','ok');}
    }
  });
}
function ruleIndexRequest(i, extra) {
  var row=(SR_STATE.ruleRows || []).filter(function(r) {return Number(r.index)===Number(i);})[0];
  if (!row) {srShowMsg('规则已变化，请刷新后重试。','err');return null;}
  var expected=Object.assign({},row);delete expected.index;
  return Object.assign({index:i,expected:expected},extra || {});
}
function moveRule(i, dir) {
  var request=ruleIndexRequest(i,{dir:dir});if(!request)return;
  ruleApi('move_rule', request, function (d) {
    toastResp(d);
  });
}
function delRule(i) {
  var request=ruleIndexRequest(i);if(!request)return;
  if (!confirm('删除这条规则？')) return;
  ruleApi('del_rule', request, function (d) {
    toastResp(d);
  });
}
function applyRuleScene(id) {
  if (!confirm('应用所选规则模板？这会改变流量分流，请确认后继续。')) return;
  ruleApi('apply_rule_scene', { scene: id }, function (d) {
    toastResp(d);
  });
}
var ruleExportPending = false;
function exportRules() {
  if (ruleExportPending) return;
  ruleExportPending = true;
  ruleApi('export_rules', null, function (d) {
    ruleExportPending = false;
    if (d.ok === false) { toastResp(d); return; }
    srElement('rule-export-text').value = d.text || '';
    srElement('rule-export-count').textContent = '共 ' + (d.count || 0) + ' 条规则。编辑中的草稿保持不变。';
    srElement('rule-export-dialog').showModal();
  }, function () { ruleExportPending = false; });
}

/* ══ Config ════════════════════════════════════════════════════════ */
var savedConfigText = null, configSaving = false, configReadGeneration = 0;
function configEdited() {
  var dirty = srElement('config-text').value !== savedConfigText;
  srElement('config-state').textContent = dirty ? '有未保存的修改' : '配置已保存';
}
function srLoadConfig() {
  if (configSaving) { srShowMsg('配置正在保存，请稍后重新加载。', 'info'); return; }
  var el = srElement('config-text');
  if (savedConfigText !== null && el.value !== savedConfigText && !confirm('放弃未保存的配置修改并重新加载？')) return;
  var before = el.value, generation = ++configReadGeneration;
  srApi('config', null, function (d) {
    if (generation !== configReadGeneration) return;
    if (d.ok === false || typeof d.config_base64 !== 'string') { srShowMsg(d.msg || '配置加载失败，请重试。','err'); return; }
    if (el.value !== before) { srShowMsg('加载期间已有新修改，未覆盖编辑内容；如需替换请重新加载。', 'info'); return; }
    savedConfigText = b64utf8(d.config_base64); el.value = savedConfigText; configEdited();
  });
}
function srSaveConfig() {
  if (configSaving) return;
  if (savedConfigText === null) { srShowMsg('请先加载现有配置，再编辑保存。','err'); return; }
  var text = srElement('config-text').value;
  // Reads begun before this write are no longer an authoritative snapshot.
  ++configReadGeneration;
  configSaving = true; srElement('config-save').disabled = true;
  srElement('config-state').textContent = '正在校验并保存…';
  function done() { configSaving = false; srElement('config-save').disabled = false; configEdited(); }
  srApi('save_config', { config: utf8b64(text) }, function (d) {
    if (d.ok !== false) savedConfigText = text;
    done(); toastResp(d); setTimeout(refreshStatus, 800);
  }, done);
}
window.addEventListener('beforeunload', function (event) {
  if (ruleWorkspaceDirty() || srElement('config-text').value !== (savedConfigText === null ? '' : savedConfigText)) { event.preventDefault(); event.returnValue = ''; }
});

var pendingUpdateAsset = null;
function srCheckUpdate() {
  var st = srElement('update-status');
  var btn = srElement('btn-apply-update');
  st.textContent = '正在检查 GitHub 发布版本…';
  btn.style.display = 'none';
  pendingUpdateAsset = null;
  srApi('check_update', null, function (d) {
    if (d.ok === false) {
      st.textContent = d.msg || '检查失败';
      srShowMsg(d.msg || '检查失败', 'err');
      return;
    }
    st.textContent = d.msg || '';
    var link = srElement('btn-update-link');
    if (d.html_url) link.href = d.html_url;
    if (d.available) {
      pendingUpdateAsset = d.asset_url || null;
      btn.style.display = pendingUpdateAsset ? '' : 'none';
      if (pendingUpdateAsset) srShowMsg(d.msg || '有可用更新', 'ok');
      else {
        st.textContent = '发现新版 v' + d.latest + '，但未找到适配当前机型的软件包。可查看发布记录。';
        srShowMsg(st.textContent, 'info');
      }
    } else {
      srShowMsg(d.msg || '已是最新版本', 'ok');
    }
  }, function () {
    st.textContent = '更新检查失败，请检查网络后重试';
    srShowMsg('更新检查失败', 'err');
  }, 70000);
}
var srUpdatePollTimer = null;
var srUpdateJobId = null;
var srUpdateLaunchDeadline = 0;
function srPollUpdate() {
  if (srUpdatePollTimer) clearTimeout(srUpdatePollTimer);
  var expected = srUpdateJobId;
  srApi('update_status', null, function (d) {
    if (expected !== srUpdateJobId) return;
    var st = srElement('update-status'), status = d.status || 'idle';
    if (expected && expected !== d.job_id) {
      if (Date.now() < srUpdateLaunchDeadline) {
        st.textContent = '正在确认本次更新任务…';
        srUpdatePollTimer = setTimeout(srPollUpdate, 1000);
      } else {
        st.textContent = '无法确认本次更新任务，请刷新页面查看当前状态。';
        srUpdateJobId = null;
      }
      return;
    }
    if (d.active) {
      srUpdateJobId = d.job_id || null;
      var labels = {checking:'核对版本和机型',downloading:'下载软件包',verifying:'校验软件包',queued:'等待开始',preparing:'准备备份',stopping:'停止服务',replacing:'安装文件',integration:'更新系统集成','api-restart':'启动管理接口','service-start':'启动代理','health-check':'检查服务'};
      st.textContent = '更新进行中：' + (labels[d.stage || status] || '处理中');
      srUpdatePollTimer = setTimeout(srPollUpdate, 2000);
      return;
    }
    srUpdateJobId = null;
    if (status === 'up-to-date') st.textContent = '已是最新版本。';
    else if (status.indexOf('success:') === 0) st.textContent = '更新成功，健康检查通过。';
    else if (status.indexOf('rolled-back:') === 0) st.textContent = '更新未通过检查，已恢复旧版并通过健康检查。';
    else if (status.indexOf('rollback-failed:') === 0) st.textContent = '恢复未通过检查，备份已保留，请检查诊断日志。';
    else if (status.indexOf('failed:') === 0) st.textContent = '更新失败：' + status.slice(7).trim();
    else st.textContent = '当前没有更新任务。';
    refreshStatus();
  }, function () {
    srElement('update-status').textContent = '等待管理接口恢复，正在重新查询更新结果…';
    srUpdatePollTimer = setTimeout(srPollUpdate, 3000);
  }, 10000);
}
function srApplyUpdate() {
  var st = srElement('update-status');
  if (!confirm('下载并安装最新完整插件？\n\n订阅、节点和规则会保留。服务将重启，网络连接可能短暂中断。')) return;
  if (srUpdateJobId) return;
  var random = new Uint8Array(16);
  window.crypto.getRandomValues(random);
  srUpdateJobId = Array.prototype.map.call(random, function (b) { return ('0' + b.toString(16)).slice(-2); }).join('');
  srUpdateLaunchDeadline = Date.now() + 30000;
  st.textContent = '正在启动更新任务…';
  srElement('btn-apply-update').style.display = 'none';
  srApi('apply_update', {request_id:srUpdateJobId}, function (d) {
    toastResp(d);
    st.textContent = d.msg || '';
    if (d.ok !== false) { srUpdateJobId = d.job_id || null; srPollUpdate(); }
    else srUpdateJobId = null;
  }, function () {
    st.textContent = '请求中断，正在查询实际更新结果…';
    srPollUpdate();
  }, 180000);
}

/* ══ Logs ══════════════════════════════════════════════════════════ */
var logAutoTimer = null, currentLogText = '', logLoading = false;
function srRenderLog() {
  var el = srElement('log-view'), atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 30;
  var filter = srElement('log-filter').value, hideReset = srElement('log-hide-reset').checked;
  el.textContent = currentLogText.split('\n').filter(function (line) {
    if (hideReset && /task error: connection reset/.test(line)) return false;
    return filter === 'all' || (filter === 'errors' ? /ERROR|panic|fatal/i.test(line) : /WARN|ERROR|panic|fatal/i.test(line));
  }).join('\n') || '没有符合筛选条件的日志';
  if (atBottom) el.scrollTop = el.scrollHeight;
}
function loadLog() {
  if (logLoading) return; logLoading = true;
  srApi('log', null, function (d) { logLoading = false; currentLogText = b64utf8(d.log_base64).replace(/\x1b\[[0-9;]*m/g, ''); srRenderLog(); }, function () { logLoading = false; });
}
function srClearLog() {
  if (!confirm('清空已保留的日志？这会删除排查联网问题所需的记录。')) return;
  srApi('logs_clear', {}, function (d) {
    toastResp(d);
    loadLog();
  });
}
function toggleLogAuto() {
  var on = srElement('log-auto').checked;
  clearInterval(logAutoTimer);
  if (on) logAutoTimer = setInterval(function () { if (SR_STATE.page === 'logs' && !document.hidden) loadLog(); }, 5000);
}

document.addEventListener('visibilitychange', function () {
  if (!document.hidden && TRANSPORT) { refreshStatus(true); if (SR_STATE.page === 'logs' && srElement('log-auto').checked) loadLog(); }
});
srElement('sidebar').addEventListener('keydown', function (event) {
  var order = SR_PAGES, index = order.indexOf(SR_STATE.page);
  if (event.key === 'ArrowRight') index = (index + 1) % order.length;
  else if (event.key === 'ArrowLeft') index = (index + order.length - 1) % order.length;
  else if (event.key === 'Home') index = 0;
  else if (event.key === 'End') index = order.length - 1;
  else return;
  event.preventDefault(); srShowPage(order[index]); srElement('nav-' + order[index]).focus();
});
['node-dialog', 'sub-dialog'].forEach(function (id) {
  srElement(id).addEventListener('close', function () {
    if (id === 'node-dialog') { srElement('detail-endpoint').textContent = '已隐藏'; srElement('detail-reveal').checked = false; }
    else { srElement('sub-detail-url').textContent = '已隐藏'; srElement('sub-detail-reveal').checked = false; inspectedSubUrl = null; }
  });
});

/* ══ Boot ══════════════════════════════════════════════════════════ */

probeTransport(function () {
  // Opening the settings page must never change the user's stopped state.
  srLoadNodes();
  refreshStatus(true);
  srPollUpdate();
  var tick = 0;
  setInterval(function () { if (!document.hidden) refreshStatus(++tick % 3 === 0); }, 10000);
});
</script>
</section></td></tr></table><div id="footer"></div>
</body>
</html>
