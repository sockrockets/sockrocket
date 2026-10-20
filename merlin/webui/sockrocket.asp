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
#sockrocket-plugin #sr-config-text {height:400px;margin-top:12px}
#sockrocket-plugin #sr-node-search {flex:1;min-width:150px;max-width:340px}
#sockrocket-plugin .config-state {font-size:12px;color:var(--text-muted);margin-top:12px}
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

/* Configuration flow overview shares the firmware-integrated light palette. */
#sockrocket-plugin .rule-flow-head {display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:8px}
#sockrocket-plugin .rule-flow-head h3 {margin:0;flex:1}
#sockrocket-plugin .flow-columns {display:grid;grid-template-columns:22% 44% 26%;gap:4%;margin:18px 0 10px;color:var(--text-muted);font-size:11px}
#sockrocket-plugin .flow-stage {position:relative;display:grid;grid-template-columns:22% 44% 26%;gap:4%;min-width:0}
#sockrocket-plugin .flow-lines {position:absolute;inset:0;width:100%;height:100%;pointer-events:none;overflow:visible}
#sockrocket-plugin .flow-lines path {fill:none;stroke:var(--border);stroke-width:1.5;vector-effect:non-scaling-stroke}
#sockrocket-plugin .flow-lines path.selected {stroke:var(--accent);stroke-width:2}
#sockrocket-plugin .flow-entry,#sockrocket-plugin .flow-rules,#sockrocket-plugin .flow-exits {position:relative;display:flex;flex-direction:column;justify-content:center;gap:10px;min-width:0}
#sockrocket-plugin .flow-box {display:flex;flex-direction:column;justify-content:center;gap:2px;min-width:0;height:68px;flex-shrink:0;padding:8px 10px;background:var(--bg-sidebar);border:1px solid var(--border);border-radius:9px;text-align:left;line-height:1.35}
#sockrocket-plugin button.flow-box {width:100%;color:var(--text-primary)}
#sockrocket-plugin button.flow-box:hover {background:var(--bg-hover);border-color:#a6c9eb}
#sockrocket-plugin .flow-box.selected {border-color:var(--accent);background:var(--accent-dim)}
#sockrocket-plugin .flow-box strong {font-size:12px;font-weight:600;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}
#sockrocket-plugin .flow-box small {display:block;font-size:10px;color:var(--text-muted);overflow:hidden;white-space:nowrap;text-overflow:ellipsis}
#sockrocket-plugin .flow-box .flow-target {font-size:10px;color:var(--text-accent);margin-left:6px;font-weight:400}
#sockrocket-plugin .flow-exits .flow-box {border-left:3px solid var(--border)}
#sockrocket-plugin .flow-exits .flow-box.selected {border-left-color:var(--accent)}
#sockrocket-plugin .flow-detail {display:flex;gap:12px;align-items:center;flex-wrap:wrap;background:var(--bg-hover);border-radius:8px;padding:12px 14px;margin-top:16px;min-width:0}
#sockrocket-plugin .flow-detail>div {flex:1;min-width:140px;overflow-wrap:anywhere}
#sockrocket-plugin .flow-detail p {font-size:11px;color:var(--text-muted)}
#sockrocket-plugin .flow-state {font-size:11px;background:var(--bg-hover);color:var(--text-muted);border-radius:5px;padding:3px 8px}
@media(max-width:600px) {
 #sockrocket-plugin .rule-flow-head h3 {flex-basis:100%}
 #sockrocket-plugin .flow-columns,#sockrocket-plugin .flow-lines {display:none}
 #sockrocket-plugin .flow-stage {display:flex;flex-direction:column;gap:14px;height:auto!important;margin-top:16px}
 #sockrocket-plugin .flow-entry::before,#sockrocket-plugin .flow-rules::before,#sockrocket-plugin .flow-exits::before {font-size:11px;color:var(--text-muted)}
 #sockrocket-plugin .flow-entry::before {content:'入口'}
 #sockrocket-plugin .flow-rules::before {content:'↓ 匹配顺序 · 首条命中即结束'}
 #sockrocket-plugin .flow-exits::before {content:'↓ 处理方式'}
 #sockrocket-plugin .flow-box strong,#sockrocket-plugin .flow-box small {white-space:normal;overflow-wrap:anywhere}
 #sockrocket-plugin .flow-box {height:auto;min-height:68px}
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
    <div class="spacer"></div>
    <button type="button" class="btn btn-danger" id="sr-exit-btn" onclick="exitProxy()" title="停止代理并恢复直连，保留订阅和规则">退出代理</button>
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

        <div class="network-summary" role="status"><span>服务进程 <b id="sr-health-process">等待状态</b></span><span>路由器探测 <b id="sr-health-outbound">尚未检查</b></span><span>局域网设备 <b>需要设备侧验证</b></span><span id="sr-status-freshness">正在读取状态…</span></div>
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
<div class="card"><h3>网络设置</h3>        <div class="toolbar">
          <button type="button" class="chip toggle" id="sr-chip-dns" onclick="toggleDns()" title="控制局域网 DNS 接管。关闭后使用运营商 DNS；开启且未启用透明代理时返回短 TTL 真实地址。">
            <span class="dot" id="sr-dns-dot"></span>DNS 接管 <b id="sr-dns-state">—</b>
          </button>
          <button type="button" class="chip toggle" id="sr-chip-fw" onclick="toggleProxy()" title="控制 TUN 透明代理。单独关闭它会保留 DNS 接管；恢复直连请使用诊断页的停止操作。">
            <span class="dot" id="sr-fw-dot"></span>透明代理 <b id="sr-fw-state">—</b>
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
        <div class="toolbar">
          <button class="btn btn-accent btn-sm" id="sr-test-all-btn" onclick="testAllNodes()">全部测速</button>
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
            <div class="fg"><label for="sr-sub-name">名称</label><input type="text" id="sr-sub-name" placeholder="例如：订阅 A"></div>
            <div class="fg"><label for="sr-sub-url">订阅地址</label><input type="url" autocomplete="off" spellcheck="false" id="sr-sub-url" placeholder="https://..."></div>
            <div class="fg"><label>格式</label>
              <select id="sr-sub-format">
                <option value="auto" selected>auto</option>
                <option value="clash">clash</option>
                <option value="v2ray">v2ray (URI / base64)</option>
                <option value="singbox">sing-box</option>
                <option value="base64">base64</option>
              </select>
            </div>
            <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" id="sr-sub-add" onclick="addSub()">添加</button></div>
          </div>
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
        <div class="card" id="sr-sub-diagnostic" hidden>
          <h3>订阅失败诊断</h3><p class="hint">以下信息不包含订阅地址、节点凭证或个人路径，可复制用于反馈。</p>
          <pre class="rule-preview" id="sr-sub-diagnostic-text"></pre>
          <button type="button" class="btn btn-sm" onclick="copySubscriptionDiagnostic()">复制脱敏诊断</button>
        </div>
      </div>

      <!-- ── Page: Routing rules ─────────────────────────────────────── -->
      <div class="page" id="sr-page-dns" role="tabpanel" aria-labelledby="sr-nav-dns">
        <div class="page-title">分流规则</div><p class="page-intro">先匹配设备专属规则，再匹配全局规则、国内直连与全局兜底。设备请使用固定 DHCP IP；更改前可预览确认。</p>
        <section class="card" aria-labelledby="sr-rule-flow-title">
          <div class="rule-flow-head"><h3 id="sr-rule-flow-title">流量去向</h3><span class="flow-state" id="sr-rule-flow-state">等待状态</span><button type="button" class="btn btn-sm" id="sr-rule-flow-toggle" aria-expanded="true" aria-controls="sr-rule-flow" onclick="toggleRuleFlow()">收起概览</button></div>
          <p class="hint">配置路径概览，非实时流量或命中追踪。自上而下匹配，首条命中即结束；点选规则可查看或编辑。</p>
          <div class="toolbar" style="margin-top:12px"><label for="sr-rule-flow-client">预览设备路径</label><input id="sr-rule-flow-client" type="text" placeholder="来源 IP，例如 192.0.2.10" autocomplete="off" style="flex:1;min-width:140px" onkeydown="if(event.key==='Enter')previewDeviceFlow()"><button type="button" class="btn btn-sm" onclick="previewDeviceFlow()">查看路径</button><button type="button" class="btn btn-sm" onclick="resetDeviceFlow()">全局规则</button></div>
          <p class="hint">这里只预览已有配置，不会绑定设备或改变网络。全局模式、直连模式会跳过设备规则。</p>
          <div id="sr-rule-flow" aria-label="分流配置路径"><p class="hint">正在加载规则…</p></div>
        </section>
        <div class="card">
<div class="toolbar"><h3 id="sr-rule-editor-state">添加规则 / 批量粘贴</h3><button class="btn btn-sm" id="sr-rule-edit-cancel" hidden onclick="cancelRuleEditor()">取消编辑</button></div>
          <datalist id="sr-rule-group-list"></datalist>
          <div class="form-grid adaptive-form" style="grid-template-columns:repeat(auto-fit,minmax(120px,1fr))">
            <div class="fg"><label for="sr-rule-group">分组</label><input type="text" id="sr-rule-group" placeholder="例如：跨境访问" list="sr-rule-group-list"></div>
            <div class="fg"><label for="sr-rule-entry-mode">编辑方式</label><select id="sr-rule-entry-mode" onchange="ruleTypeChanged()"><option value="single">单条添加</option><option value="bulk">批量粘贴</option></select></div>
            <div class="fg"><label>类型</label>
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
            <div class="fg"><label>处理方式</label>
              <select id="sr-rule-target">
                <option value="proxy">代理</option>
                <option value="direct">直连</option>
                <option value="reject">拒绝</option>
              </select>
            </div>
            <div class="fg"><label for="sr-rule-source">设备来源 IP / 网段</label><input type="text" id="sr-rule-source" placeholder="留空为全局规则" autocomplete="off"><span class="hint">填写固定 DHCP IP；设备组可填 CIDR。Merlin LAN 透明接管仅支持 IPv4。</span></div>
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
              <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" onclick="previewRuleChanges()">添加</button></div>
            </div>
          </div>
          <div id="sr-rule-bulk-wrap">
            <textarea id="sr-rule-batch" rows="5" spellcheck="false" placeholder="DOMAIN-SUFFIX,example.invalid,PROXY&#10;GEOIP,CN,DIRECT&#10;MATCH,PROXY"></textarea>
            <div class="toolbar" style="margin-top:8px">
              <div class="spacer"></div>
              <span class="hint">域名、网段、地区可每行一个值；混合规则使用 TYPE,内容,ACTION。</span>
              <button class="btn btn-success btn-sm" onclick="previewRuleChanges()">预览并应用</button>
            </div>
          </div>
        </div>
        <div class="card">
          <div class="toolbar">
            <h3 style="margin:0">规则列表</h3><button class="btn btn-sm" onclick="exportRules()">导出</button><button class="btn btn-sm" onclick="srLoadRules()">刷新</button>
            <div class="spacer"></div>
            <button class="btn btn-sm" onclick="applyRuleScene('cross_border')">跨境访问模板</button>
            <button class="btn btn-sm" onclick="applyRuleScene('domestic_direct')">国内直连模板</button>
          </div>
          <div class="rule-filters"><input type="search" id="sr-rule-search" aria-label="搜索规则" placeholder="搜索域名、网段或分组" oninput="renderRules()"><select id="sr-rule-filter-target" aria-label="筛选处理方式" onchange="renderRules()"><option value="">全部处理方式</option><option value="proxy">代理</option><option value="direct">直连</option><option value="reject">拒绝</option></select><select id="sr-rule-filter-state" aria-label="筛选状态" onchange="renderRules()"><option value="">全部状态</option><option value="enabled">已启用</option><option value="disabled">已停用</option></select></div><p class="hint" id="sr-rule-count" aria-live="polite"></p>
          <div class="rows" id="sr-rule-list"><div class="hint">正在加载…</div></div>
          <div class="toolbar" id="sr-rule-pager" hidden><span class="hint" id="sr-rule-page-label"></span><div class="spacer"></div><button class="btn btn-sm" id="sr-rule-page-prev" onclick="changeRulePage(-1)">上一页</button><button class="btn btn-sm" id="sr-rule-page-next" onclick="changeRulePage(1)">下一页</button></div>
          <div class="hint">按优先级从高到低匹配；相同优先级按配置顺序匹配。设备最终规则只兜底对应设备，优先于全局规则；全局最终规则在内置国内直连之后。上下移动仅限同一匹配阶段，会更新规则优先级，并针对完整列表操作，不受筛选或分页影响。</div>
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
          <div class="hint" id="sr-update-status">检查 GitHub 发布版本，仅替换程序文件； <strong>config.yaml</strong>、订阅、节点和规则会保留。</div>
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

<dialog id="sr-rule-edit-dialog" aria-labelledby="sr-rule-edit-title"><h2 id="sr-rule-edit-title">编辑规则</h2><p class="hint">可直接修改优先级，数字越大越先匹配；改为最终规则时会在国内直连之后匹配。运行中的服务会重启以应用规则。</p><div class="form-grid"><div class="fg"><label for="sr-edit-rule-group">分组</label><input type="text" id="sr-edit-rule-group" list="sr-rule-group-list"></div><div class="fg"><label for="sr-edit-rule-type">类型</label><select id="sr-edit-rule-type" onchange="editRuleTypeChanged()"><option value="domain">完整域名</option><option value="domain-suffix">域名后缀</option><option value="domain-keyword">域名关键词</option><option value="ip-cidr">IP 网段</option><option value="geoip">地区</option><option value="dst-port">端口</option><option value="final">最终规则</option></select></div><div class="fg"><label for="sr-edit-rule-pattern">匹配内容</label><input type="text" id="sr-edit-rule-pattern" spellcheck="false"></div><div class="fg"><label for="sr-edit-rule-source">设备来源 IP / 网段</label><input id="sr-edit-rule-source" type="text" placeholder="留空为全局规则" autocomplete="off"></div><div class="fg"><label for="sr-edit-rule-priority">优先级（数字越大越先匹配）</label><input type="number" id="sr-edit-rule-priority" step="1" min="-2147483648" max="2147483647"></div><div class="fg"><label for="sr-edit-rule-target">处理方式</label><select id="sr-edit-rule-target"><option value="proxy">代理</option><option value="direct">直连</option><option value="reject">拒绝</option></select></div></div><label><input type="checkbox" id="sr-edit-rule-enabled"> 启用此规则</label><div class="dialog-actions"><button class="btn" onclick="srElement('rule-edit-dialog').close()">取消</button><button class="btn btn-accent" id="sr-rule-edit-save" onclick="saveRuleEdit()">保存规则</button></div></dialog>
<dialog id="sr-rule-dialog" aria-labelledby="sr-rule-dialog-title"><h2 id="sr-rule-dialog-title">规则详情</h2><pre id="sr-rule-detail-text" class="rule-preview"></pre><div class="dialog-actions"><button class="btn" onclick="srElement('rule-dialog').close()">关闭</button></div></dialog>
<dialog id="sr-rule-preview-dialog" aria-labelledby="sr-rule-preview-title"><h2 id="sr-rule-preview-title">确认规则更改</h2><p class="hint" id="sr-rule-preview-summary"></p><pre id="sr-rule-preview-text" class="rule-preview"></pre><p class="hint">确认后由路由器校验并保存；任一行格式有误将不保存。运行中的服务会重启以应用规则，可能短暂影响连接。</p><div class="dialog-actions"><button class="btn" onclick="srElement('rule-preview-dialog').close()">返回编辑</button><button class="btn btn-accent" onclick="applyRulePreview()">确认应用</button></div></dialog>

<dialog id="sr-rule-export-dialog" aria-labelledby="sr-rule-export-title"><h2 id="sr-rule-export-title">导出规则</h2><p class="hint" id="sr-rule-export-count"></p><textarea id="sr-rule-export-text" readonly aria-label="导出的规则" spellcheck="false"></textarea><div class="dialog-actions"><button class="btn" onclick="srElement('rule-export-dialog').close()">关闭</button><button class="btn btn-accent" onclick="srCopyText(srElement('rule-export-text').value)">复制规则</button></div></dialog>

<script>
/* ══ API ═══════════════════════════════════════════════════════════ */
/* Three transports, auto-detected at boot:
 *   'http' — sockrocket-cli's built-in API server (http://<router>:18188/api),
 *            always-on, works whether or not the proxy daemon is running
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
  if (TRANSPORT === 'ksc') return apiKsc(action, data, cb, errCb, timeoutMs);
  return apiXhr(TRANSPORT === 'http' ? apiBase() : '/cgi-bin/sockrocket.cgi', action, data, cb, errCb, timeoutMs);
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
        srShowMsg('错误：' + (d.error_message || d.error || d.error_code), 'err');
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
  if (action === 'stop') return apiKscRequest(action, data, cb, errCb, timeoutMs);
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
  fields['sockrocket_rpc_body_' + seq] = JSON.stringify(data || {});
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
          srShowMsg('错误：' + (d.error_message || d.error || d.error_code), 'err');
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
var SR_PAGES = ['home', 'nodes', 'subs', 'dns', 'logs', 'config'];
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
    if (p === 'nodes') srLoadNodes();
    if (p === 'subs') srLoadSubs();
    if (p === 'dns') { srLoadRules(); ruleTypeChanged(); }
    if (p === 'config') srLoadConfig();
    if (p === 'logs') loadLog();
  } else {
    if (p === 'nodes') renderNodes();
  }
}

/* ══ Title bar / status bar / home ═════════════════════════════════ */
var refreshPending = {};
function refreshStatus(withSysinfo) {
  function read(action, success) {
    if (refreshPending[action]) return;
    refreshPending[action] = true;
    srApi(action, null, function (d) { refreshPending[action] = false; success(d); }, function () {
      refreshPending[action] = false;
      if (action === 'status') {
        SR_STATE.statusKnown = false;
        srElement('status-freshness').textContent = '状态读取失败，显示的是上次结果';
        renderChrome();
      }
    });
  }
  read('status', function (d) {
    if (typeof d.running !== 'boolean') { SR_STATE.statusKnown = false; renderChrome(); return; }
    SR_STATE.status = d; SR_STATE.statusKnown = true; SR_STATE.statusAt = Date.now();
    srElement('status-freshness').textContent = '更新于 ' + new Date().toLocaleTimeString();
    renderChrome(); renderNodes();
  });
  read('stats', function (d) { SR_STATE.stats = d; renderChrome(); });
  if (withSysinfo) read('sysinfo', function (d) { SR_STATE.sysinfo = d; renderChrome(); });
}

function renderChrome() {
  var st = SR_STATE.status, ss = SR_STATE.stats, si = SR_STATE.sysinfo || {};
  var running = !!st.running;
  var pill = srElement('service-pill');
  pill.textContent = !SR_STATE.statusKnown ? '状态未知' : running ? '运行中' : '已停止';
  pill.className = 'service-pill' + (SR_STATE.statusKnown && running ? ' success' : '');
  srElement('chip-dns').disabled = !SR_STATE.statusKnown;
  srElement('chip-fw').disabled = !SR_STATE.statusKnown;

  srElement('health-process').textContent = !SR_STATE.statusKnown ? '状态未知' : running ? '运行正常' : '已停止';
  var probe = st.health || {}, age = Date.now() / 1000 - Number(probe.last_probe_at || 0);
  srElement('health-outbound').textContent = !SR_STATE.statusKnown ? '状态未知' : !running ? '未运行' : age >= 0 && age < 120
    ? (probe.consecutive_failures > 0 ? '最近探测失败' : '最近检查通过 · ' + Math.round(age) + ' 秒前') : '近期没有探测结果';
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
  var exitBtn = srElement('exit-btn');
  exitBtn.disabled = serviceStopPending || (SR_STATE.statusKnown && !running && !servicePending);
  exitBtn.textContent = serviceStopPending ? '正在退出…' : '退出代理';
  var tb = srElement('toggle-btn');
  tb.disabled = servicePending || !SR_STATE.statusKnown;
  tb.textContent = servicePending ? '正在处理…' : !SR_STATE.statusKnown ? '等待状态' : running ? '停止服务' : '启动服务';
  tb.className = 'btn ' + (running ? 'btn-danger' : 'btn-success');

  // chips: the config keys are the source of truth (they survive a service
  // stop), unlike the runtime stats which go inactive with the daemon.
  setChip('dns', st.dns_hijack);
  setChip('fw', st.transparent_proxy);
  setProxyModeUi(st.proxy_mode || 'rule');
  if (SR_STATE.page === 'dns') renderRuleFlow();
  var dnsChip = srElement('chip-dns');
  var fwChip = srElement('chip-fw');
  if (dnsChip) {
    dnsChip.title = st.dns_hijack
      ? (st.transparent_proxy
          ? 'DNS 接管与 TUN 已开启，使用短 TTL Fake-IP'
          : 'DNS 接管已开启，使用短 TTL 真实地址')
      : 'DNS 接管已关闭，局域网使用运营商 DNS';
  }
  if (fwChip) {
    fwChip.title = st.transparent_proxy
      ? '透明代理已开启；单独关闭它会保留 DNS 接管'
      : '透明代理已关闭，DNS 接管由单独开关控制';
  }
  srElement('wan-ip').textContent = si.wan_ip || '—';

  // stat cards
  srElement('st-mem').textContent = si.mem_pct != null ? si.mem_pct + '%' : '—';
  srElement('st-mem-sub').textContent = si.mem_used_kb ? Math.round(si.mem_used_kb / 1024) + ' / ' + Math.round(si.mem_total_kb / 1024) + ' MB' : '';
  srElement('st-load').textContent = si.load || '—';
  srElement('st-uptime').textContent = si.uptime ? '路由器运行 ' + si.uptime : '';
}

function setChip(id, on) {
  var dot = srElement(id + '-dot');
  if (!dot) return;
  dot.className = 'dot ' + (on ? 'ok' : 'off');
  srElement(id + '-state').textContent = on ? '已开启' : '已关闭';
}

/* Chips are switches now: the API call flips the key, then the next status
 * poll renders the result. While in flight the chip is disabled — rapid
 * clicking used to fire overlapping service restarts. */
function chipPending(id, on) {
  var el = srElement(id);
  if (el) el.classList.toggle('pending', on);
}
function toggleDns() {
  if (!SR_STATE.statusKnown || srElement('chip-dns').classList.contains('pending')) return;
  var want = !SR_STATE.status.dns_hijack;
  // DNS hijack without TUN is OK (short-TTL real IPs, no Fake-IP).
  chipPending('chip-dns', true);
  srApi('set_toggles', { dns_hijack: want }, function (d) {
    chipPending('chip-dns', false);
    toastResp(d);
    refreshStatus(true);
  }, function () { chipPending('chip-dns', false); });
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
  if (!SR_STATE.statusKnown || srElement('chip-fw').classList.contains('pending')) return;
  var want = !SR_STATE.status.transparent_proxy;
  chipPending('chip-fw', true);
  // Enabling TUN also ensures DNS hijack (Fake-IP needs it). Disabling TUN
  // leaves DNS on so clients keep short-TTL answers and recover in seconds.
  var body = want
    ? { transparent_proxy: true, dns_hijack: true }
    : { transparent_proxy: false };
  if (want) chipPending('chip-dns', true);
  srApi('set_toggles', body, function (d) {
    chipPending('chip-fw', false);
    chipPending('chip-dns', false);
    toastResp(d);
    setTimeout(function () { refreshStatus(true); }, want ? 3500 : 2000);
  }, function () {
    chipPending('chip-fw', false);
    chipPending('chip-dns', false);
  });
}

/* ══ Service control ═══════════════════════════════════════════════ */
var servicePending = false, serviceInFlight = 0, serviceStopPending = false, serviceGeneration = 0;
function serviceAction(action) {
  // Stop bypasses the lifecycle gate, just as it bypasses the RPC read queue.
  if (action === 'stop' ? serviceStopPending : servicePending) return;
  var generation = ++serviceGeneration;
  serviceInFlight++; servicePending = true;
  if (action === 'stop') serviceStopPending = true;
  renderChrome();
  function done(d) {
    serviceInFlight--; servicePending = serviceInFlight > 0;
    if (action === 'stop') serviceStopPending = false;
    // An older start/restart cannot replace a newer stop's feedback.
    if (generation === serviceGeneration && d) toastResp(d);
    renderChrome(); refreshStatus(true);
  }
  srApi(action, {}, done, function () { done(); }, 90000);
}

function exitProxy() {
  if (!confirm('停止路由器代理并恢复直连？订阅、节点和规则会保留。')) return;
  serviceAction('stop');
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
      return '<div class="diag-row"><span class="d-name">' + name + '</span><span class="' + (ok ? 'check-ok' : 'check-bad') + '">' + (ok ? '✓ ' : '— ') + srEsc(detail) + '</span></div>';
    }
    var html = row('服务进程', d.running, d.running ? '运行中' : '已停止') +
      row('配置校验', d.config_valid, d.config_valid ? '通过' : '未通过') +
      row('透明代理规则', d.iptables_rules > 0, Number(d.iptables_rules || 0) + ' 条标记规则') +
      row('DNS 接管', d.dns_rules > 0, d.dns_rules > 0 ? '已启用' : '未发现接管规则') +
      row('dnsmasq', d.dnsmasq_running, d.dnsmasq_running ? '运行中' : '未运行') +
      row('防火墙链', d.fw_active, d.fw_active ? '已挂载' : '未挂载') +
      '<p class="hint">未启用的功能不一定表示故障，请结合网络设置判断。局域网设备联网需要在设备上确认。</p>';
    srElement('diag-body').innerHTML = html;
  }, function () { done(); srElement('diag-body').textContent = '检查请求失败，请确认路由器管理连接后重试。'; });
}
function restoreDirect() {
  if (!confirm('停止路由器代理并恢复直连？海外网站可能暂时无法访问，订阅、节点和规则会保留。')) return;
  serviceAction('stop');
}

/* ══ Nodes ═════════════════════════════════════════════════════════ */
function srLoadNodes() {
  srApi('node_details', null, function (d) {
    if (d.ok === false || !Array.isArray(d.nodes)) { srShowMsg(d.msg || '节点加载失败，请重试。','err'); return; }
    var previous = Object.create(null);
    SR_STATE.nodes.forEach(function (n,i) { if (SR_STATE.lat[i] != null) previous[nodeViewKey(n)] = SR_STATE.lat[i]; });
    SR_STATE.nodes = d.nodes; SR_STATE.lat = {};
    SR_STATE.nodes.forEach(function(n,i) { var key = nodeViewKey(n); if (previous[key] != null) SR_STATE.lat[i] = previous[key]; });
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
function nodeViewKey(n) { return JSON.stringify([n.name, n.protocol, n.server, n.port]); }
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
    renderNodes();
    renderChrome();
  }, function () {
    if (!SR_STATE.nodes[i] || nodeViewKey(SR_STATE.nodes[i]) !== key) return;
    SR_STATE.lat[i] = -1;
    renderNodes();
    renderChrome();
  }, 60000);
}
function testAllNodes() {
  if (SR_STATE.testingAll || !SR_STATE.nodes.length) return;
  SR_STATE.testingAll = true;
  var btn = srElement('test-all-btn');
  btn.disabled = true; btn.textContent = '正在测速…';
  // Keep concurrency low on Merlin: each probe is a full protocol handshake
  // and the API also caps at 2 in-process. Higher parallelism starved the
  // router CPU and produced mass false timeouts.
  var CONCURRENCY = 2;
  var keys = SR_STATE.nodes.map(nodeViewKey);
  var next = 0;
  var inflight = 0;
  function finishAll() {
    SR_STATE.testingAll = false;
    btn.disabled = false;
    btn.textContent = '全部测速';
  }
  function pump() {
    if (SR_STATE.page !== 'nodes' || document.hidden || JSON.stringify(SR_STATE.nodes.map(nodeViewKey)) !== JSON.stringify(keys)) { finishAll(); return; }
    while (inflight < CONCURRENCY && next < SR_STATE.nodes.length) {
      (function (idx) {
        inflight++;
        var el = srElement('lat-' + idx);
        if (el) el.textContent = '…';
        function done(ok, d) {
          if (SR_STATE.nodes[idx] && nodeViewKey(SR_STATE.nodes[idx]) === keys[idx]) SR_STATE.lat[idx] = ok && d && d.ok && d.latency >= 0 ? d.latency : -1;
          renderNodes();
          inflight--;
          if (next >= SR_STATE.nodes.length && inflight === 0) finishAll();
          else pump();
        }
        srApi('speedtest_node', { index: idx },
          function (d) { done(true, d); },
          function () { done(false, null); },
          60000);
      })(next++);
    }
  }
  pump();
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
    el.innerHTML = subs.map(function (s) {
      return '<div class="row">' +
        '<span class="badge acc">' + srEsc(s.format || 'auto') + '</span>' +
        '<span class="r-name">' + srEsc(s.name) + '<span class="r-sub">订阅地址已隐藏</span></span>' +
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
function subscriptionBusy(on) { subscriptionPending = on; srElement('sub-add').disabled = on; srElement('sub-update').disabled = on; }
function scheduleSubscriptionPoll() {
  clearTimeout(subscriptionJobTimer);
  subscriptionJobTimer = setTimeout(function () { pollSubscriptionJob(subscriptionJobId); }, 2000);
}
function subscriptionErrorText(d, fallback) {
  var text = d && (d.msg || d.error_message || d.error || d.error_code) || fallback;
  if (d && d.stage) text += ' [' + d.stage + (d.cause ? ': ' + d.cause : '') + ']';
  if (d && d.error_code) text += '\n错误码：' + d.error_code;
  if (d && typeof d.os_error === 'number') text += '\n系统错误码：' + d.os_error;
  if (d && d.hint) text += '\n建议：' + d.hint;
  return text;
}
function showSubscriptionDiagnostic(d, jobId) {
  d = d || {};
  var stages = {prepare_directory:'准备数据目录',lock_job:'获取任务锁',save_request:'保存订阅请求',save_job:'保存任务状态',resolve_worker:'定位后台程序',spawn_worker:'启动后台进程',fetching:'下载订阅',applying:'保存并应用配置'};
  var causes = {PermissionDenied:'权限不足',AlreadyExists:'路径已被其他文件占用',NotADirectory:'数据路径不是目录',IsADirectory:'文件路径实际为目录',NotFound:'程序或路径缺失',StorageFull:'存储空间不足',ReadOnlyFilesystem:'文件系统只读',WouldBlock:'资源暂被占用',Unsupported:'操作或文件系统能力不支持',Other:'系统调用失败'};
  var codes = ['subscription_start_failed','subscription_job_not_found','subscription_worker_exited','timeout','network_error','http_error','parse_error','busy','dispatch_error'];
  var code = codes.indexOf(d.error_code) >= 0 ? d.error_code : 'subscription_failed';
  var reason = causes[d.cause] || (code === 'timeout' ? '请求超时' : code === 'network_error' ? '无法连接插件接口' : code === 'busy' ? '接口正在处理其他请求' : '订阅任务未完成，请结合阶段与日志排查');
  var hint = d.cause === 'PermissionDenied' ? '检查插件数据目录及程序权限，保留已有配置。' : d.cause === 'StorageFull' || d.os_error === 28 || d.os_error === 122 ? '检查并释放路由器存储空间。' : d.cause === 'ReadOnlyFilesystem' || d.os_error === 30 ? '检查存储挂载及文件系统错误。' : '请反馈错误码、失败阶段和对应时间的脱敏日志。';
  var status = /\bHTTP(?:\/\d(?:\.\d)?)?\s+([1-5]\d{2})\b/.exec(String(d.msg || d.error || ''));
  if (status) {
    reason = '订阅服务器返回 HTTP ' + status[1];
    hint = /^(401|403)$/.test(status[1]) ? '核对订阅权限、有效期及服务端访问限制。' : status[1] === '429' ? '服务端限制请求频率，请稍后重试。' : '核对订阅服务器状态及链接有效性。';
  }
  // Only allowlisted metadata enters the copyable report. Raw messages may
  // contain server content or personal paths and are deliberately excluded.
  var lines = ['错误码：' + code,'失败阶段：' + (stages[d.stage] || '未确认'),'原因：' + reason];
  if (typeof d.os_error === 'number' && isFinite(d.os_error)) lines.push('系统错误码：' + d.os_error);
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
    scheduleSubscriptionPoll(); return;
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
  subscriptionJobId = job.id;
  if (job.state === 'running') {
    subscriptionBusy(true);
    var counts = job.total ? '（' + Number(job.completed || 0) + '/' + Number(job.total) + '）' : '';
    srShowMsg((job.stage === 'applying' ? '正在保存并应用订阅' : '正在后台下载订阅') + counts + '，关闭页面后仍会继续。', 'info');
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
  if (result.ok === false) { showSubscriptionDiagnostic(result, job.id); srShowMsg(subscriptionErrorText(result, '订阅任务失败'), 'err'); }
  else { srElement('sub-diagnostic').hidden = true; toastResp(result); }
  srLoadSubs(); srLoadNodes(); refreshStatus();
}
function pollSubscriptionJob(id) {
  srApi('subscription_job', id ? {id:id} : {}, acceptSubscriptionJob, function (error) {
    if (error && error.error_code === 'subscription_job_not_found') { subscriptionJobMissing(); return; }
    srShowMsg('暂时无法读取订阅进度，正在重试；刷新页面也可继续查看。', 'info');
    scheduleSubscriptionPoll();
  }, 12000);
}
function restoreSubscriptionJob() {
  if (subscriptionJobChecked) return;
  subscriptionJobChecked = true;
  srApi('subscription_job', {}, function (d) {
    if (d.ok !== false && d.job && !subscriptionPending) {
      if (d.job.state === 'running') acceptSubscriptionJob(d);
      else if (d.job.result && d.job.result.ok === false) showSubscriptionDiagnostic(d.job.result, d.job.id);
    }
  }, function () { subscriptionJobChecked = false; }, 12000);
}
function startSubscriptionJob(action, body) {
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
  if (!/^https?:\/\//i.test(data.url)) { srShowMsg('请输入 http 或 https 订阅地址', 'err'); return; }
  if (!data.name || !data.url) { srShowMsg('名称和订阅地址不能为空', 'err'); return; }
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

/* ══ Routing rules ═════════════════════════════════════════════════ */
var RULE_TYPE_LABEL = { 'domain': '完整域名', 'domain-suffix': '域名后缀', 'domain-keyword': '域名关键词', 'ip-cidr': 'IP 网段', 'geoip': '地区', 'dst-port': '端口', 'final': '最终规则', 'match': '最终规则' };
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
  if (ta) ta.placeholder = RULE_BULK_PLACEHOLDER[t] || '';
  var hint = srElement('rule-input-hint');
  if (hint) {
    hint.textContent = bulk
      ? (RULE_EDIT_GROUP !== null ? '编辑整个分组：每行 TYPE,内容,ACTION。格式有误时不会保存。' : ((RULE_BULK_HINT[t] || '每行一条 TYPE,内容,ACTION；最终规则为 MATCH,ACTION。') + ' 也可粘贴混合类型的完整规则。'))
      : (t === 'final'
          ? '处理未命中的流量，无需填写匹配内容'
          : (t === 'geoip'
              ? RULE_GEOIP_HINT
              : (RULE_PATTERN_HINT[t] ? ('示例：' + RULE_PATTERN_HINT[t]) : '')));
  }
  srElement('rule-source').disabled = RULE_EDIT_GROUP !== null;
  var dup = srElement('rule-dup');
  if (dup && dup.parentElement) dup.parentElement.style.display = bulk ? '' : 'none';
}
var RULE_COLLAPSED = Object.create(null);
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
function isFinalRule(r) { return r.rule_type === 'match' || r.rule_type === 'final'; }
function rulePhase(r) {return (r.source ? 0 : 2)+Number(isFinalRule(r));}
function orderedRules(rules) {
  return rules.slice().sort(function(a,b) {
    return rulePhase(a) - rulePhase(b) || Number(b.priority || 0) - Number(a.priority || 0) || Number(a.index) - Number(b.index);
  });
}
var RULE_FLOW_SELECTED = 0, RULE_FLOW_ERROR = false, RULE_FLOW_CLIENT = '';
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
  return !!(net&&ip&&net.width===ip.width&&(net.value>>BigInt(net.width-net.prefix))===(ip.value>>BigInt(net.width-net.prefix)));
}
function previewDeviceFlow() {
  var text=srElement('rule-flow-client').value.trim(), ip=sourceNetwork(text);
  if(!ip || text.indexOf('/')>=0) {srShowMsg('请输入要预览的单个 IPv4 或 IPv6 地址。','err');return;}
  RULE_FLOW_CLIENT=text;RULE_FLOW_SELECTED=0;renderRuleFlow();
}
function resetDeviceFlow() {RULE_FLOW_CLIENT='';srElement('rule-flow-client').value='';RULE_FLOW_SELECTED=0;renderRuleFlow();}
function ruleFlowModel(rows, mode, client) {
  if (mode === 'global' || mode === 'direct') return [{title:mode === 'global' ? '全局代理' : '全部直连', note:'当前模式跳过分流规则', targets:[mode === 'global' ? 'proxy' : 'direct']}];
  var active = orderedRules(rows).filter(function(r) {return r.enabled !== false && (!r.source || (client && sourceContains(r.source,client)));});
  var deviceFinals=active.filter(function(r){return r.source&&isFinalRule(r);}),deviceFinal=deviceFinals[0];
  var custom = active.filter(function(r) {return !isFinalRule(r) && (!deviceFinal || r.source);});
  var finals = deviceFinal ? deviceFinals : active.filter(isFinalRule), result = [];
  function item(r, label) {return {title:label || r.name || r.group || r.pattern || '自定义规则', note:(isFinalRule(r) ? '最终规则' : (RULE_TYPE_LABEL[r.rule_type] || r.rule_type))+' · 优先级 '+Number(r.priority || 0)+(r.source ? ' · '+r.source : ' · 全局'), targets:[r.target], rule:r};}
  custom.slice(0,4).forEach(function(r,i) {var row=item(r);row.title=(i+1)+'. '+row.title;result.push(row);});
  if (custom.length > 4) {
    var targets=[];custom.slice(4).forEach(function(r) {if(targets.indexOf(r.target)<0)targets.push(r.target);});
    result.push({title:'其余 '+(custom.length-4)+' 条自定义规则',note:'仍按优先级依次匹配 · 查看完整列表',targets:targets,more:true});
  }
  if(!deviceFinal)result.push({title:'内置国内 / 局域网',note:'国内域名、CN IP 与私有网段',targets:['direct']});
  if (finals.length) {
    var finalRow=item(finals[0],deviceFinal ? '设备兜底' : '最终兜底');
    finalRow.note='未命中的其余流量 · 优先级 '+Number(finals[0].priority || 0)+(finals.length>1 ? ' · 后续 '+(finals.length-1)+' 条最终规则被覆盖' : '');result.push(finalRow);
  } else result.push({title:'默认兜底',note:'未命中的其余流量',targets:['proxy']});
  return result;
}
function selectRuleFlow(index) {RULE_FLOW_SELECTED=index;renderRuleFlow();var button=srElement('rule-flow-step-'+RULE_FLOW_SELECTED);if(button)button.focus();}
function toggleRuleFlow() {
  var host=srElement('rule-flow'), button=srElement('rule-flow-toggle');
  host.hidden=!host.hidden;button.textContent=host.hidden ? '展开概览' : '收起概览';button.setAttribute('aria-expanded',String(!host.hidden));
}
function showFlowRuleList() {
  srElement('rule-search').value='';srElement('rule-filter-target').value='';srElement('rule-filter-state').value='';
  renderRules();srElement('rule-list').scrollIntoView({behavior:'smooth',block:'start'});
}
function renderRuleFlow() {
  var host=srElement('rule-flow');if(!host)return;
  var st=SR_STATE.status || {}, mode=String(st.proxy_mode || 'rule').toLowerCase();
  var known=SR_STATE.statusKnown, running=!!st.running;
  srElement('rule-flow-state').textContent=(!known ? '状态未知' : running ? '服务运行中' : '服务已停止')+' · '+(mode==='global' ? '全局模式' : mode==='direct' ? '直连模式' : '规则模式');
  if(!Array.isArray(SR_STATE.ruleRows)) {host.innerHTML='<p class="hint">'+(RULE_FLOW_ERROR ? '规则加载失败，请刷新后重试。' : '正在加载规则…')+'</p>';return;}
  var rows=ruleFlowModel(SR_STATE.ruleRows,mode,RULE_FLOW_CLIENT), h=Math.max(224,rows.length*78-10);
  RULE_FLOW_SELECTED=Math.max(0,Math.min(RULE_FLOW_SELECTED,rows.length-1));
  var selected=rows[RULE_FLOW_SELECTED], targets=['proxy','direct','reject'];
  var node=(SR_STATE.nodes || [])[st.active_node];
  var names={proxy:'代理',direct:'直连',reject:'拒绝'};
  var descriptions={proxy:node ? node.name : '当前所选节点',direct:'不经过代理节点',reject:'阻止连接'};
  function curve(x1,y1,x2,y2,on) {var mid=(x1+x2)/2;return '<path'+(on ? ' class="selected"' : '')+' d="M '+x1+' '+y1+' C '+mid+' '+y1+', '+mid+' '+y2+', '+x2+' '+y2+'"/>';}
  var paths='', startY=(h-(rows.length*78-10))/2;
  rows.forEach(function(r,i) {
    var y=startY+i*78+34,on=i===RULE_FLOW_SELECTED;
    paths+=curve(220,h/2,260,y,on);
    r.targets.forEach(function(t) {var j=targets.indexOf(t);if(j>=0)paths+=curve(700,y,740,(h-224)/2+j*78+34,on);});
  });
  var boxRows=rows.map(function(r,i) {return '<button type="button" id="sr-rule-flow-step-'+i+'" class="flow-box'+(i===RULE_FLOW_SELECTED ? ' selected' : '')+'" aria-pressed="'+(i===RULE_FLOW_SELECTED)+'" onclick="selectRuleFlow('+i+')"><strong title="'+srEsc(r.title)+'">'+srEsc(r.title)+'</strong><small>'+srEsc(r.note)+'</small><small class="flow-target">'+r.targets.map(function(t) {return srEsc(names[t] || t);}).join(' / ')+'</small></button>';}).join('');
  var outlets=targets.map(function(t) {return '<div class="flow-box'+(selected.targets.indexOf(t)>=0 ? ' selected' : '')+'"><strong>'+names[t]+'</strong><small title="'+srEsc(descriptions[t])+'">'+srEsc(descriptions[t])+'</small></div>';}).join('');
  var detail=selected.rule ? (RULE_TYPE_LABEL[selected.rule.rule_type] || selected.rule.rule_type)+' · '+(isFinalRule(selected.rule) ? '未命中的其余流量' : selected.rule.pattern)+' · 优先级 '+Number(selected.rule.priority || 0)+' · 来源 '+(selected.rule.source || '全局') : selected.note;
  var actions=selected.rule ? '<button type="button" class="btn btn-sm" onclick="inspectRule('+Number(selected.rule.index)+')">详情</button><button type="button" class="btn btn-accent btn-sm" onclick="editRule('+Number(selected.rule.index)+')">编辑规则</button>' : selected.more ? '<button type="button" class="btn btn-sm" onclick="showFlowRuleList()">查看完整列表</button>' : '';
  host.innerHTML='<div class="flow-columns" aria-hidden="true"><span>入口</span><span>匹配顺序 · 首条命中即结束</span><span>出口</span></div><div class="flow-stage" style="height:'+h+'px"><svg class="flow-lines" viewBox="0 0 1000 '+h+'" preserveAspectRatio="none" aria-hidden="true">'+paths+'</svg><div class="flow-entry"><div class="flow-box"><strong>'+srEsc(RULE_FLOW_CLIENT || '全局规则路径')+'</strong><small>设备流量是否进入此路径取决于接管设置</small></div></div><div class="flow-rules">'+boxRows+'</div><div class="flow-exits">'+outlets+'</div></div><div class="flow-detail" aria-live="polite"><div><strong>'+srEsc(selected.title)+'</strong><p>'+srEsc(detail)+'</p></div>'+actions+'</div><p class="hint">'+(!known ? '服务状态读取失败或尚未确认；以上仅展示已读取的配置。' : !running ? '服务已停止，以上仅展示配置，不代表流量正在经过代理。' : mode!=='rule' ? '当前模式不使用规则列表；切回规则模式后按优先级分流。' : (RULE_FLOW_CLIENT ? (rows.some(function(r){return r.rule && r.rule.source && isFinalRule(r.rule);}) ? '设备兜底覆盖后续全局规则，不再继续匹配。仅展示已启用项。' : '先匹配该设备与重叠网段的专属规则，未命中时回退至全局规则。') : '当前仅展示全局规则。输入设备 IP，可预览对应设备与网段策略。'))+(RULE_FLOW_ERROR ? ' 规则刷新失败，当前展示上次读取结果。' : '')+'</p>';
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
    '<div class="rule-meta">配置 #' + (Number(r.index) + 1) + ' · 来源 ' + srEsc(r.source || '全局') + ' · 优先级 ' + Number(r.priority || 0) + ' · ' + (r.enabled === false ? '已停用' : '已启用') + '</div></div>' +
    '<div class="rule-actions"><button class="btn btn-sm" onclick="inspectRule(' + Number(r.index) + ')">详情</button>' +
    '<button class="btn btn-sm" onclick="editRule(' + Number(r.index) + ')">编辑</button>' +
    '<button class="btn btn-sm" onclick="moveRule(' + Number(r.index) + ',-1)" aria-label="上移规则"' + edge(-1) + '>↑</button>' +
    '<button class="btn btn-sm" onclick="moveRule(' + Number(r.index) + ',1)" aria-label="下移规则"' + edge(1) + '>↓</button>' +
    '<button class="btn btn-danger btn-sm" onclick="delRule(' + Number(r.index) + ')">删除</button></div></div>';
}
var rulePending = false;
function ruleApi(action, data, cb, errCb) {
  if (action === 'get_rules' || action === 'export_rules') return srApi(action, data, cb, errCb);
  if (rulePending) { srShowMsg('正在保存规则，请稍候。','err'); return; }
  rulePending = true;
  function done() { rulePending = false; }
  srShowMsg('正在保存并应用规则，请稍候…','info');
  srApi(action, data, function(d) { done(); if (cb) cb(d); }, function() { done(); srShowMsg('请求未完成，请刷新确认保存状态后再重试。草稿已保留。','err'); if (errCb) errCb(); }, 60000);
}
function srLoadRules() {
  ruleApi('get_rules', null, function (d) {
    var el = srElement('rule-list');
    if (d.ok === false || !Array.isArray(d.rules)) { RULE_FLOW_ERROR = true; renderRuleFlow(); srShowMsg(d.msg || '规则加载失败，请重试。','err'); return; }
    var rules = d.rules;
    SR_STATE.ruleRows = rules; RULE_FLOW_ERROR = false;
    refreshRuleGroupList(rules);
    renderRules(false);
  }, function () { RULE_FLOW_ERROR = true; renderRuleFlow(); SR_PAGE_LOADED['dns'] = false; srElement('rule-list').textContent = '加载失败，请点击刷新或重新打开此页。'; });
}
var RULE_PAGE = 0, RULE_PAGE_SIZE = 50;
function changeRulePage(dir) { RULE_PAGE += dir; renderRules(false); }
function renderRules(resetPage) {
    renderRuleFlow();
    if (resetPage !== false) RULE_PAGE = 0;
    var el = srElement('rule-list'), all = SR_STATE.ruleRows || [];
    var query = srElement('rule-search').value.trim().toLowerCase();
    var target = srElement('rule-filter-target').value, state = srElement('rule-filter-state').value;
    var sorted = orderedRules(all), positions = Object.create(null);
    sorted.forEach(function(r,i) { positions[r.index] = i; });
    var rules = sorted.filter(function(r) {
      return (!query || [r.pattern,r.source,r.group,r.name,RULE_TYPE_LABEL[r.rule_type]].join(' ').toLowerCase().indexOf(query) >= 0) &&
        (!target || r.target === target) && (!state || (state === 'disabled' ? r.enabled === false : r.enabled !== false));
    });
    srElement('rule-count').textContent = '共 ' + all.length + ' 条 · 已启用 ' + all.filter(function(r) {return r.enabled !== false;}).length + ' 条 · 当前显示 ' + rules.length + ' 条';
    var pages = Math.max(1, Math.ceil(rules.length / RULE_PAGE_SIZE));
    RULE_PAGE = Math.max(0, Math.min(RULE_PAGE, pages - 1));
    srElement('rule-pager').hidden = rules.length <= RULE_PAGE_SIZE;
    srElement('rule-page-label').textContent = '第 ' + (RULE_PAGE + 1) + ' / ' + pages + ' 页 · 每页最多 ' + RULE_PAGE_SIZE + ' 条';
    srElement('rule-page-prev').disabled = RULE_PAGE === 0;
    srElement('rule-page-next').disabled = RULE_PAGE === pages - 1;
    rules = rules.slice(RULE_PAGE * RULE_PAGE_SIZE, (RULE_PAGE + 1) * RULE_PAGE_SIZE);
    if (!rules.length) { el.innerHTML = '<div class="hint">' + (all.length ? '没有匹配的规则，请调整筛选条件。' : '尚无自定义规则，当前使用内置国内直连与代理兜底。可添加规则或使用模板。') + '</div>'; return; }
    var buckets = groupRulesForDisplay(rules);
    el.innerHTML = buckets.map(function (b) {
      var key = b.name;
      var title = key || '未分组';
      var collapsed = !!RULE_COLLAPSED[key];
      var enabledN = b.indices.filter(function (i) { return rules[i].enabled !== false; }).length;
      var body = b.indices.map(function (i) { return ruleRowHtml(rules[i], positions[rules[i].index], sorted); }).join('');
      var gEnc = encodeURIComponent(key).replace(/'/g, '%27');
      return (!rules[b.indices[0]].source && isFinalRule(rules[b.indices[0]]) && (b.indices[0] === 0 || !isFinalRule(rules[b.indices[0]-1])) ? '<div class="rule-phase">内置国内直连规则 → 以下为最终兜底规则</div>' : '') + '<div class="rule-group' + (collapsed ? ' collapsed' : '') + '">' +
        '<div class="rule-group-hd">' +
          '<button class="btn btn-sm" aria-label="展开或折叠分组" aria-expanded="' + (!collapsed) + '" onclick="toggleRuleGroup(\'' + gEnc + '\')">' + (collapsed ? '▸' : '▾') + '</button>' +
          '<span class="rg-title">' + srEsc(title) + '</span>' +
          '<span class="rg-count">本段启用 ' + enabledN + '/' + b.indices.length + '</span>' +
          '<span class="rg-actions" onclick="event.stopPropagation()">' +
            '<button class="btn btn-sm" onclick="useRuleGroup(\'' + gEnc + '\')">使用分组</button>' +
            '<button class="btn btn-sm" onclick="editRuleGroup(\'' + gEnc + '\')">批量编辑整个分组</button>' +
            '<button class="btn btn-sm" onclick="setRuleGroupEnabled(\'' + gEnc + '\',true)">启用</button>' +
            '<button class="btn btn-sm" onclick="setRuleGroupEnabled(\'' + gEnc + '\',false)">停用</button>' +
            '<button class="btn btn-danger btn-sm" onclick="deleteRuleGroup(\'' + gEnc + '\')">删除</button>' +
          '</span>' +
        '</div>' +
        '<div class="rule-group-body">' + body + '</div></div>';
    }).join('');
}
function useRuleGroup(enc) {
  srElement('rule-group').value = decodeURIComponent(enc);
}
function toggleRuleGroup(enc) {
  var key = decodeURIComponent(enc);
  RULE_COLLAPSED[key] = !RULE_COLLAPSED[key];
  renderRules(false);
}
function setRuleGroupEnabled(enc, enabled) {
  ruleApi('set_rule_group_enabled', { group: decodeURIComponent(enc), enabled: !!enabled }, function (d) {
    toastResp(d);
    if (d.ok !== false) srLoadRules();
  });
}
function deleteRuleGroup(enc) {
  var name = decodeURIComponent(enc) || '未分组';
  if (!confirm('删除分组「' + name + '」内的全部规则？')) return;
  ruleApi('delete_rule_group', { group: decodeURIComponent(enc) }, function (d) {
    toastResp(d);
    if (d.ok !== false) srLoadRules();
  });
}
function inspectRule(index) {
  var rule = (SR_STATE.ruleRows || []).filter(function (r) { return Number(r.index) === Number(index); })[0];
  if (!rule) return;
  srElement('rule-detail-text').textContent = '来源：' + (rule.source || '全局') + '\n类型：' + (RULE_TYPE_LABEL[rule.rule_type] || rule.rule_type) + '\n匹配：' + rule.pattern + '\n处理：' + (RULE_TARGET_LABEL[rule.target] || rule.target) + '\n优先级：' + Number(rule.priority || 0) + '\n分组：' + (rule.group || '未分组') + '\n状态：' + (rule.enabled === false ? '已停用' : '已启用');
  srElement('rule-dialog').showModal();
}
var RULE_EDIT_ROW = null;
function editRule(index) {
  var r = (SR_STATE.ruleRows || []).filter(function(r) { return Number(r.index) === Number(index); })[0];
  if (!r) return;
  RULE_EDIT_ROW = JSON.parse(JSON.stringify(r));
  ['source','group','pattern','target','priority'].forEach(function(k) { srElement('edit-rule-' + k).value = r[k] == null ? '' : r[k]; });
  srElement('edit-rule-type').value = r.rule_type === 'match' ? 'final' : r.rule_type;
  srElement('edit-rule-enabled').checked = r.enabled !== false;
  editRuleTypeChanged(); srElement('rule-edit-dialog').showModal();
}
function editRuleTypeChanged() {
  var t = srElement('edit-rule-type').value;
  srElement('edit-rule-pattern').disabled = t === 'final';
  srElement('edit-rule-pattern').placeholder = RULE_PATTERN_HINT[t] || '';
}
function saveRuleEdit() {
  if (!RULE_EDIT_ROW || rulePending) return;
  var priority = readRulePriority('edit-rule-priority'); if (priority === false || priority === null) { srShowMsg('请填写有效的整数优先级。','err'); return; }
  var original = RULE_EDIT_ROW, expected = Object.assign({}, original); delete expected.index; expected.source=original.source || '';
  var data = {index:original.index, expected:expected, source:srElement('edit-rule-source').value.trim(), rule_type:srElement('edit-rule-type').value,
    pattern:srElement('edit-rule-pattern').value.trim(), group:srElement('edit-rule-group').value.trim(),
    priority:priority, target:srElement('edit-rule-target').value, enabled:srElement('edit-rule-enabled').checked};
  if (data.rule_type === 'final') data.pattern = '*';
  if (!data.pattern) { srShowMsg('请输入匹配内容。','err'); return; }
  srElement('rule-edit-save').disabled = true;
  ruleApi('update_rule',data,function(d) {
    srElement('rule-edit-save').disabled = false; toastResp(d);
    if (d.ok !== false) { if (RULE_EDIT_ROW === original) { srElement('rule-edit-dialog').close(); RULE_EDIT_ROW = null; } srLoadRules(); }
  },function() { srElement('rule-edit-save').disabled = false; });
}
function cancelRuleEditor() {
  if (confirm('放弃当前未保存的规则草稿？')) resetRuleEditor();
}
function resetRuleEditor() {
  srElement('rule-priority').value = '0'; srElement('rule-source').value='';
  RULE_EDIT_GROUP = null; RULE_EDIT_MEMBERS = null; srElement('rule-entry-mode').disabled = false; srElement('rule-group').value = ''; srElement('rule-batch').value = ''; srElement('rule-pattern').value = '';
  srElement('rule-editor-state').textContent = '添加规则 / 批量粘贴'; srElement('rule-edit-cancel').hidden = true; ruleTypeChanged();
}
var rulePreviewSnapshot = null;
function ruleEditorSnapshot() { return JSON.stringify(['rule-source','rule-entry-mode','rule-group','rule-type','rule-target','rule-dup','rule-priority','rule-batch','rule-pattern'].map(function(id) { return srElement(id).value; }).concat([RULE_EDIT_GROUP])); }
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
  srElement('rule-preview-text').textContent = text || 'MATCH,' + srElement('rule-target').value.toUpperCase();
  srElement('rule-preview-summary').textContent = '分组：' + (srElement('rule-group').value.trim() || '未分组') + (RULE_EDIT_GROUP !== null ? '。这会替换整个分组，并将其集中到首次出现的位置；未修改规则的启用状态和优先级会保留。' : '。类型：' + (RULE_TYPE_LABEL[srElement('rule-type').value] || '混合') + '；默认处理：' + RULE_TARGET_LABEL[srElement('rule-target').value] + '。现有规则按所选重复处理方式保留或替换。');
  srElement('rule-preview-summary').textContent += ' 优先级：'+(readRulePriority('rule-priority') === null ? (RULE_EDIT_GROUP !== null ? '保留原值，新规则沿用分组优先级' : '保留原值，新规则为 0') : readRulePriority('rule-priority'))+'（数字越大越先匹配）。';
  srElement('rule-preview-summary').textContent += ' 来源：'+(RULE_EDIT_GROUP !== null ? '保留每行 SOURCE 或原分组来源' : srElement('rule-source').value.trim() || '全局 / 每行 SOURCE')+'。';
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
  if (!members.length) return;
  if ((RULE_EDIT_GROUP !== null || srElement('rule-batch').value.trim() || srElement('rule-pattern').value.trim()) && !confirm('批量编辑会替换当前未保存的草稿，是否继续？')) return;
  srElement('rule-priority').value = '';
  RULE_EDIT_GROUP = key;
  RULE_EDIT_MEMBERS = members.map(function(r) { var copy = Object.assign({},r); delete copy.index; return copy; });
  srElement('rule-entry-mode').value = 'bulk';
  srElement('rule-editor-state').textContent = '正在编辑分组：' + (key || '未分组');
  srElement('rule-edit-cancel').hidden = false;
  srElement('rule-editor-state').scrollIntoView({block:'center',behavior:'smooth'});
  srElement('rule-group').value = key;
  var typ = members[0].rule_type === 'match' ? 'final' : members[0].rule_type;
  srElement('rule-type').value = typ;
  srElement('rule-target').value = members[0].target;
  function clashLine(r) {
    var T = {
      'domain': 'DOMAIN', 'domain-suffix': 'DOMAIN-SUFFIX', 'domain-keyword': 'DOMAIN-KEYWORD',
      'ip-cidr': 'IP-CIDR', 'geoip': 'GEOIP', 'dst-port': 'DST-PORT', 'port': 'DST-PORT',
      'match': 'MATCH', 'final': 'MATCH'
    };
    var act = (r.target || 'proxy').toUpperCase();
    var line = (r.rule_type === 'match' || r.rule_type === 'final') ? 'MATCH,'+act : (T[r.rule_type] || r.rule_type.toUpperCase())+','+r.pattern+','+act;
    return r.source ? 'SOURCE,'+r.source+','+line : line;
  }
  var text = members.map(clashLine).join('\n');
  ruleTypeChanged();
  srElement('rule-bulk-wrap').style.display = '';
  srElement('rule-single-wrap').style.display = 'none';
  srElement('rule-batch').value = text;
  srElement('rule-pattern').value = '';
  var hint = srElement('rule-input-hint');
  if (hint) hint.textContent = '编辑分组「' + (key || '未分组') + '」：每行 TYPE,pattern,ACTION；设备规则保留 SOURCE,来源, 前缀。应用后替换整个分组。';
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
        if (draft === ruleEditorSnapshot()) resetRuleEditor();
        srLoadRules();
      }
    });
    return;
  }
  if (!grp) { srShowMsg('请输入分组名称，规则按分组维护', 'err'); return; }
  if (srElement('rule-entry-mode').value === 'bulk') {
    var text = srElement('rule-batch').value;
    if (!text.trim()) { srShowMsg('请先粘贴至少一条规则', 'err'); return; }
    ruleApi('batch_add_rules', withPriority({ text: text, target: g, group: grp, rule_type: t, replace: replace }), function (d) {
      toastResp(d);
      if (d.ok !== false) {
        if (srElement('rule-batch').value === text) srElement('rule-batch').value = '';
        srLoadRules();
      }
    });
    return;
  }
  var p = srElement('rule-pattern').value.trim();
  if (t !== 'final' && !p) { srShowMsg('请输入匹配内容', 'err'); return; }
  ruleApi('add_rule', { group: grp, source:srElement('rule-source').value.trim(), rule_type: t, pattern: p, target: g, priority: priority === null ? 0 : priority }, function (d) {
    toastResp(d);
    if (d.ok !== false) {
      if (srElement('rule-pattern').value === p) srElement('rule-pattern').value = '';
      srLoadRules();
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
    if (d.ok !== false) srLoadRules();
  });
}
function delRule(i) {
  var request=ruleIndexRequest(i);if(!request)return;
  if (!confirm('删除这条规则？')) return;
  ruleApi('del_rule', request, function (d) {
    toastResp(d);
    if (d.ok !== false) srLoadRules();
  });
}
function applyRuleScene(id) {
  if (!confirm('应用所选规则模板？这会改变流量分流，请确认后继续。')) return;
  ruleApi('apply_rule_scene', { scene: id }, function (d) {
    toastResp(d);
    if (d.ok !== false) srLoadRules();
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
  if (srElement('config-text').value !== (savedConfigText === null ? '' : savedConfigText)) { event.preventDefault(); event.returnValue = ''; }
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
      srShowMsg(d.msg || '有可用更新', 'ok');
    } else {
      srShowMsg(d.msg || '已是最新版本', 'ok');
    }
  }, function () {
    st.textContent = '更新检查失败，请检查网络后重试';
    srShowMsg('更新检查失败', 'err');
  }, 30000);
}
function srApplyUpdate() {
  var st = srElement('update-status');
  if (!confirm('下载并安装最新程序？\n\n订阅、节点和规则会保留。服务将重启，网络连接可能短暂中断。')) return;
  st.textContent = '正在下载并安装，可能需要一分钟…';
  srElement('btn-apply-update').style.display = 'none';
  srApi('apply_update', null, function (d) {
    toastResp(d);
    st.textContent = d.msg || '';
    setTimeout(refreshStatus, 1500);
  }, function () {
    st.textContent = '安装失败，请检查网络后重试';
    srShowMsg('安装失败', 'err');
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
  var tick = 0;
  setInterval(function () { if (!document.hidden) refreshStatus(++tick % 3 === 0); }, 10000);
});
</script>
</section></td></tr></table><div id="footer"></div>
</body>
</html>
