<!DOCTYPE html>
<html>
<head>
<meta http-equiv="Content-Type" content="text/html; charset=utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="Pragma" content="no-cache">
<meta http-equiv="Expires" content="-1">
<link rel="shortcut icon" href="images/favicon.png">
<title>Sockrocket</title>
<!-- Written by sockrocket.sh (api-start): defines SOCKROCKET_API_PORT from config.yaml's
     api_port. Missing on koolcenter / old installs — the code falls back. -->
<script src="api.js"></script>
<style>
/* ── Sockrocket design tokens (mirrors the desktop GUI theme) ─────────────── */
:root {
  --bg-app: #080a10;
  --bg-sidebar: #0d1117;
  --bg-hover: #131922;
  --border: rgba(30, 40, 55, .6);
  --text-primary: #f0f2f5;
  --text-muted: #4a5568;
  --text-secondary: #8b94a3;
  --accent: #22d3ee;
  --accent-dim: rgba(34, 211, 238, .1);
  --text-accent: #7ee7f8;
  --success: #34d399;
  --warning: #fbbf24;
  --danger: #f87171;
  --mono: "SF Mono", "Cascadia Mono", Consolas, Menlo, monospace;
}
* { box-sizing: border-box; margin: 0; padding: 0; }
html, body { height: 100%; }
body {
  background: var(--bg-app);
  color: var(--text-primary);
  font: 13px/1.5 -apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif;
  overflow: hidden;
  user-select: none;
}
#app { height: 100%; display: flex; flex-direction: column; }

/* ── Title bar (34px) ──────────────────────────────────────────────── */
#titlebar {
  height: 34px; flex-shrink: 0;
  display: flex; align-items: center; gap: 10px;
  padding: 0 12px 0 10px;
  background: var(--bg-sidebar);
}
#titlebar .logo { display: flex; align-items: center; gap: 7px; font-weight: 700; font-size: 13px; }
#titlebar .logo svg { color: var(--accent); }
#titlebar .spacer { flex: 1; }
.node-badge {
  display: flex; align-items: center; gap: 6px; max-width: 320px;
  font: 11px var(--mono); color: var(--text-secondary);
  border: 1px solid var(--border); border-radius: 6px; padding: 3px 9px;
  background: var(--bg-app); cursor: pointer;
}
.node-badge:hover { background: var(--bg-hover); }
.node-badge .nb-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-primary); }
.node-badge .nb-lat { color: var(--accent); flex-shrink: 0; }
.node-badge .nb-lat.bad { color: var(--danger); }

/* ── Main row: icon rail + content ─────────────────────────────────── */
#main { flex: 1; display: flex; min-height: 0; }
#sidebar {
  width: 38px; flex-shrink: 0;
  display: flex; flex-direction: column; align-items: center;
  background: var(--bg-sidebar); border-right: 1px solid var(--border);
  padding-top: 8px; gap: 4px;
}
#sidebar .nav-btn {
  position: relative; width: 32px; height: 32px; border-radius: 8px;
  display: flex; align-items: center; justify-content: center;
  color: var(--text-muted); cursor: pointer;
}
#sidebar .nav-btn:hover { background: var(--bg-hover); }
#sidebar .nav-btn.active { background: var(--accent-dim); color: var(--accent); }
#sidebar .nav-btn.active::before {
  content: ""; position: absolute; left: -3px; top: 9px;
  width: 2px; height: 14px; border-radius: 0 2px 2px 0; background: var(--accent);
}
#sidebar .nav-btn svg { width: 16px; height: 16px; }
#sidebar .nav-sep { width: 20px; height: 1px; background: var(--border); margin: 4px 0; }
#content { flex: 1; min-width: 0; overflow-y: auto; padding: 16px; }
.page { display: none; max-width: 896px; margin: 0 auto; }
.page.active { display: block; }
.page-title { font-size: 17px; font-weight: 600; margin-bottom: 12px; }

/* ── Status bar (26px) ─────────────────────────────────────────────── */
#statusbar {
  height: 26px; flex-shrink: 0;
  display: flex; align-items: center; gap: 12px;
  padding: 0 12px; background: var(--bg-sidebar); border-top: 1px solid var(--border);
  font: 10px var(--mono); color: var(--text-muted); white-space: nowrap;
}
#statusbar .sb-left { display: flex; align-items: center; gap: 6px; min-width: 0; }
#statusbar .sb-right { margin-left: auto; display: flex; gap: 12px; flex-shrink: 0; }
#statusbar .ok { color: var(--success); }
#statusbar .acc { color: var(--accent); }
#statusbar .bad { color: var(--danger); }

/* ── Cards ─────────────────────────────────────────────────────────── */
.card {
  background: var(--bg-sidebar); border: 1px solid var(--border);
  border-radius: 8px; padding: 14px 16px; margin-bottom: 12px;
}
.card h3 {
  font-size: 10px; color: var(--text-muted); text-transform: uppercase;
  letter-spacing: 1px; margin-bottom: 10px; font-weight: 600;
}
.stat-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 10px; }
.stat {
  background: var(--bg-sidebar); border: 1px solid var(--border);
  border-radius: 8px; padding: 12px 14px;
}
.stat .label { font-size: 10px; color: var(--text-muted); text-transform: uppercase; letter-spacing: .8px; }
.stat .value { font-size: 22px; font-weight: 700; font-family: var(--mono); margin-top: 4px; }
.stat .sub { font-size: 10px; color: var(--text-muted); font-family: var(--mono); margin-top: 2px; }
.stat .value.acc { color: var(--accent); }
.stat .value.ok { color: var(--success); }
.stat .value.bad { color: var(--danger); }

/* ── Hero status card ──────────────────────────────────────────────── */
.hero { display: flex; align-items: center; gap: 14px; }
.hero .hero-icon {
  width: 44px; height: 44px; border-radius: 50%; flex-shrink: 0;
  display: flex; align-items: center; justify-content: center;
  background: var(--accent-dim); color: var(--accent);
  border: 1px solid rgba(34, 211, 238, .2);
}
.hero .hero-icon.off { background: rgba(74, 85, 104, .12); color: var(--text-muted); border-color: var(--border); }
.hero .hero-icon svg { width: 20px; height: 20px; }
.hero .hero-info { flex: 1; min-width: 0; }
.hero .hero-title { display: flex; align-items: center; gap: 8px; font-size: 15px; font-weight: 600; }
.hero .hero-title .live {
  font-size: 9px; padding: 1px 6px; border-radius: 4px; font-weight: 600;
  background: var(--accent-dim); color: var(--accent); border: 1px solid rgba(34, 211, 238, .25);
}
.hero .hero-sub { font-size: 11px; color: var(--text-muted); font-family: var(--mono); margin-top: 2px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.hero .hero-metrics { display: flex; gap: 18px; margin-top: 8px; font: 11px var(--mono); color: var(--text-muted); }
.hero .hero-metrics b { color: var(--accent); font-weight: 600; }

/* ── Buttons / pills / chips ───────────────────────────────────────── */
.btn {
  padding: 6px 14px; border: 1px solid var(--border); border-radius: 6px;
  background: var(--bg-hover); color: var(--text-primary);
  cursor: pointer; font-size: 12px; font-weight: 600; transition: opacity .15s;
}
.btn:hover { opacity: .8; }
.btn:disabled { opacity: .4; cursor: default; }
.btn-accent { background: var(--accent-dim); border-color: rgba(34, 211, 238, .3); color: var(--accent); }
.btn-danger { background: rgba(248, 113, 113, .1); border-color: rgba(248, 113, 113, .3); color: var(--danger); }
.btn-success { background: rgba(52, 211, 153, .1); border-color: rgba(52, 211, 153, .3); color: var(--success); }
.btn-sm { padding: 3px 9px; font-size: 11px; border-radius: 5px; }
.chip {
  display: inline-flex; align-items: center; gap: 6px;
  border: 1px solid var(--border); border-radius: 6px; padding: 6px 11px;
  background: var(--bg-sidebar); font-size: 11px; color: var(--text-secondary);
}
.chip .dot { width: 6px; height: 6px; border-radius: 50%; }
.chip .dot.ok { background: var(--success); }
.chip .dot.off { background: var(--text-muted); }
.chip .dot.acc { background: var(--accent); }
.chip b { color: var(--text-primary); font-weight: 600; }
.chip.toggle { cursor: pointer; transition: border-color .15s, background .15s; }
.chip.toggle:hover { border-color: var(--accent); background: var(--bg-hover); }
.chip.toggle.pending { opacity: .6; pointer-events: none; }
.chip.toggle.pending b::after { content: ' …'; }

/* ── Tables / rows ─────────────────────────────────────────────────── */
.rows { display: flex; flex-direction: column; }
.row {
  position: relative; display: flex; align-items: center; gap: 10px;
  padding: 7px 10px; border-radius: 6px; cursor: pointer;
}
.row:hover { background: var(--bg-hover); }
.row.active { background: var(--accent-dim); }
.row.active::before {
  content: ""; position: absolute; left: 0; top: 8px; bottom: 8px;
  width: 2px; border-radius: 2px; background: var(--accent);
}
.row .r-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
.row .r-sub { font: 10px var(--mono); color: var(--text-muted); flex-shrink: 0; }
.badge {
  font-size: 9px; padding: 1px 6px; border-radius: 4px; font-weight: 700;
  background: var(--bg-hover); color: var(--text-secondary); flex-shrink: 0;
  text-transform: uppercase; letter-spacing: .3px;
}
.badge.acc { background: var(--accent-dim); color: var(--accent); }
.lat { font: 11px var(--mono); width: 62px; text-align: right; color: var(--text-muted); flex-shrink: 0; }
.lat.good { color: var(--success); } .lat.mid { color: var(--warning); } .lat.bad { color: var(--danger); }

/* ── Forms ─────────────────────────────────────────────────────────── */
input[type=text], input[type=url], input[type=number], select, textarea {
  background: var(--bg-app); border: 1px solid var(--border); border-radius: 6px;
  color: var(--text-primary); padding: 6px 9px; font-size: 12px; outline: none;
}
input:focus, select:focus, textarea:focus { border-color: rgba(34, 211, 238, .4); }
select option { background: var(--bg-sidebar); }
.form-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 8px; margin-bottom: 8px; }
.form-grid .fg { display: flex; flex-direction: column; gap: 3px; }
.form-grid .fg label { font-size: 10px; color: var(--text-muted); }
textarea { width: 100%; height: 380px; font: 11px var(--mono); resize: vertical; user-select: text; }

/* ── Log view ──────────────────────────────────────────────────────── */
#log-view, .log-block {
  background: #05070b; border: 1px solid var(--border); border-radius: 8px;
  padding: 12px; height: 420px; overflow-y: auto;
  font: 11px var(--mono); color: #58a6ff; white-space: pre-wrap; word-break: break-all;
  user-select: text;
}

/* ── Message toast ─────────────────────────────────────────────────── */
#msg {
  display: none; position: fixed; top: 44px; right: 16px; z-index: 99;
  padding: 8px 14px; border-radius: 6px; font-size: 12px; max-width: 420px;
  border: 1px solid var(--border); background: var(--bg-sidebar);
  box-shadow: 0 4px 16px rgba(0, 0, 0, .5);
}
#msg.ok { border-color: rgba(52, 211, 153, .4); color: var(--success); }
#msg.err { border-color: rgba(248, 113, 113, .4); color: var(--danger); }
#msg.info { border-color: rgba(34, 211, 238, .4); color: var(--text-accent); }

/* ── Misc ──────────────────────────────────────────────────────────── */
.toolbar { display: flex; align-items: center; gap: 8px; margin-bottom: 10px; flex-wrap: wrap; }
.toolbar .spacer { flex: 1; }
.diag-row { display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 12px; }
.diag-row .d-name { flex: 1; color: var(--text-secondary); }
.check-ok { color: var(--success); } .check-bad { color: var(--danger); }
.collapsible { display: none; }
.collapsible.open { display: block; }
.hint { font-size: 10px; color: var(--text-muted); margin-top: 6px; }
::-webkit-scrollbar { width: 8px; height: 8px; }
::-webkit-scrollbar-thumb { background: var(--bg-hover); border-radius: 4px; }
::-webkit-scrollbar-track { background: transparent; }
</style>
</head>
<body>
<div id="app">
  <!-- ── Title bar ─────────────────────────────────────────────────── -->
  <div id="titlebar">
    <div class="logo">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 13.5 C7 9 8.6 5.2 12 3 C15.4 5.2 17 9 17 13.5"/><path d="M5.5 15.5 C8.5 14.4 15.5 14.4 18.5 15.5"/><path d="M10 17.5 L10 19.5"/><path d="M14 17.5 L14 21.5"/></svg>
      Sockrocket
    </div>
    <div class="spacer"></div>
    <div class="node-badge" id="node-badge" onclick="showPage('nodes')" title="Click to switch node">
      <span class="nb-name" id="nb-name">—</span>
      <span class="nb-lat" id="nb-lat"></span>
    </div>
  </div>

  <!-- ── Main row ──────────────────────────────────────────────────── -->
  <div id="main">
    <div id="sidebar">
      <div class="nav-btn active" id="nav-home" title="Overview" onclick="showPage('home')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.75 6A2.25 2.25 0 016 3.75h2.25A2.25 2.25 0 0110.5 6v2.25a2.25 2.25 0 01-2.25 2.25H6a2.25 2.25 0 01-2.25-2.25V6zM14 6a2.25 2.25 0 012.25-2.25H18A2.25 2.25 0 0120.25 6v2.25A2.25 2.25 0 0118 10.5h-2.25A2.25 2.25 0 0114 8.25V6zM3.75 15.75A2.25 2.25 0 016 13.5h2.25a2.25 2.25 0 012.25 2.25V18a2.25 2.25 0 01-2.25 2.25H6A2.25 2.25 0 013.75 18v-2.25zM13.5 15.75A2.25 2.25 0 0115.75 13.5H18a2.25 2.25 0 012.25 2.25V18a2.25 2.25 0 01-2.25 2.25h-2.25A2.25 2.25 0 0113.5 18v-2.25z"/></svg>
      </div>
      <div class="nav-btn" id="nav-nodes" title="Nodes" onclick="showPage('nodes')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 21a9.004 9.004 0 008.716-6.747M12 21a9.004 9.004 0 01-8.716-6.747M12 21c2.485 0 4.5-4.03 4.5-9S14.485 3 12 3m0 18c-2.485 0-4.5-4.03-4.5-9S9.515 3 12 3m0 0a8.997 8.997 0 017.843 4.582M12 3a8.997 8.997 0 00-7.843 4.582m15.686 0A11.953 11.953 0 0112 10.5c-2.998 0-5.74-1.1-7.843-2.918m15.686 0A8.959 8.959 0 0121 12c0 .778-.099 1.533-.284 2.253m0 0A17.919 17.919 0 0112 16.5c-3.162 0-6.133-.815-8.716-2.247m0 0A9.015 9.015 0 013 12c0-1.605.42-3.113 1.157-4.418"/></svg>
      </div>
      <div class="nav-btn" id="nav-subs" title="Subscriptions" onclick="showPage('subs')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 11a9 9 0 019 9M4 4a16 16 0 0116 16M5 19a1 1 0 100-2 1 1 0 000 2z"/></svg>
      </div>
      <div class="nav-btn" id="nav-dns" title="Routing rules" onclick="showPage('dns')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a15 15 0 010 18M12 3a15 15 0 000 18"/></svg>
      </div>
      <div class="nav-btn" id="nav-config" title="Config" onclick="showPage('config')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 20l-5.447-2.724A1 1 0 013 16.382V5.618a1 1 0 011.447-.894L9 7m0 13l6-3m-6 3V7m6 10l5.447 2.724A1 1 0 0021 18.382V7.618a1 1 0 00-.553-.894L15 7m0 13V7"/></svg>
      </div>
      <div class="nav-btn" id="nav-logs" title="Logs" onclick="showPage('logs')">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6.75 7.5l3 2.25-3 2.25m4.5 0h3m-9 8.25h13.5A2.25 2.25 0 0021 18V6a2.25 2.25 0 00-2.25-2.25H5.25A2.25 2.25 0 003 6v12a2.25 2.25 0 002.25 2.25z"/></svg>
      </div>
    </div>

    <div id="content">
      <!-- ── Page: Home ────────────────────────────────────────────── -->
      <div class="page active" id="page-home">
        <div class="card">
          <div class="hero">
            <div class="hero-icon off" id="hero-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M3.75 13.5l10.5-11.25L12 10.5h8.25L9.75 21.75 12 13.5H3.75z"/></svg>
            </div>
            <div class="hero-info">
              <div class="hero-title"><span id="hero-state">Stopped</span><span class="live" id="hero-live" style="display:none">Live</span></div>
              <div class="hero-sub" id="hero-node">—</div>
              <div class="hero-sub" id="hero-health" style="display:none"></div>
              <div class="hero-metrics">
                <span>Uptime <b id="hero-uptime">—</b></span>
                <span>Nodes <b id="hero-nodes">0</b></span>
                <span>Subs <b id="hero-subs">0</b></span>
              </div>
            </div>
            <button class="btn btn-success" id="toggle-btn" onclick="toggleService()">Start service</button>
          </div>
        </div>

        <div class="toolbar">
          <div class="chip toggle" id="chip-dns" onclick="toggleDns()" title="Click to toggle DNS hijack">
            <span class="dot" id="dns-dot"></span>DNS hijack <b id="dns-state">—</b>
          </div>
          <div class="chip toggle" id="chip-fw" onclick="toggleProxy()" title="Click to toggle transparent proxy">
            <span class="dot" id="fw-dot"></span>Transparent proxy <b id="fw-state">—</b>
          </div>
          <div class="chip toggle" id="chip-cn" onclick="toggleCn()" title="China traffic bypasses TUN via hardware NAT (custom rules for China domains are ignored while enabled)">
            <span class="dot" id="cn-dot"></span>CN direct <b id="cn-state">—</b>
          </div>
          <div class="chip"><span class="dot acc"></span>Egress IP <b id="wan-ip">—</b></div>
        </div>

        <div class="stat-grid">
          <div class="stat">
            <div class="label">Node latency</div>
            <div class="value acc" id="st-lat">—</div>
            <div class="sub" id="st-lat-sub">Click to test</div>
          </div>
          <div class="stat">
            <div class="label">Memory usage</div>
            <div class="value" id="st-mem">—</div>
            <div class="sub" id="st-mem-sub">—</div>
          </div>
          <div class="stat">
            <div class="label">System load</div>
            <div class="value" id="st-load">—</div>
            <div class="sub" id="st-uptime">—</div>
          </div>
          <div class="stat">
            <div class="label">Egress IP / Region</div>
            <div class="value" id="st-ip" style="font-size:15px">—</div>
            <div class="sub" id="st-ip-sub">Click to check</div>
          </div>
        </div>

        <div class="toolbar" style="margin-top:12px">
          <button class="btn btn-accent btn-sm" onclick="runSpeedtest()">Latency test</button>
          <button class="btn btn-sm" onclick="runIpCheck()">IP check</button>
          <button class="btn btn-sm" onclick="restartService()">Restart service</button>
          <div class="spacer"></div>
          <button class="btn btn-sm" id="diag-btn" onclick="toggleDiag()">Run diagnostics</button>
        </div>

        <div class="card collapsible" id="diag-card">
          <h3>Diagnostics</h3>
          <div id="diag-body"><div class="hint">Click "Run diagnostics" to check service, firewall and DNS status.</div></div>
        </div>
      </div>

      <!-- ── Page: Nodes ───────────────────────────────────────────── -->
      <div class="page" id="page-nodes">
        <div class="page-title">Nodes</div>
        <div class="toolbar">
          <button class="btn btn-accent btn-sm" id="test-all-btn" onclick="testAllNodes()">Test all</button>
          <button class="btn btn-sm" onclick="document.getElementById('add-node-card').classList.toggle('open')">Add node</button>
          <div class="spacer"></div>
          <span class="hint" id="nodes-count"></span>
        </div>
        <div class="card collapsible" id="add-node-card">
          <h3>Add node</h3>
          <div class="form-grid">
            <div class="fg"><label>Name</label><input type="text" id="an-name" placeholder="My Node"></div>
            <div class="fg"><label>Protocol</label>
              <select id="an-proto">
                <option value="shadowsocks">shadowsocks</option>
                <option value="vmess">vmess</option>
                <option value="trojan">trojan</option>
                <option value="vless">vless</option>
                <option value="hysteria2">hysteria2</option>
                <option value="tuic">tuic</option>
              </select>
            </div>
            <div class="fg"><label>Server</label><input type="text" id="an-server" placeholder="example.com"></div>
            <div class="fg"><label>Port</label><input type="number" id="an-port" placeholder="443"></div>
            <div class="fg"><label>Password</label><input type="text" id="an-pass" placeholder="(ss/trojan/hy2/tuic)"></div>
            <div class="fg"><label>UUID</label><input type="text" id="an-uuid" placeholder="(vmess/vless/tuic)"></div>
            <div class="fg"><label>Cipher</label><input type="text" id="an-cipher" placeholder="(ss/vmess, default aes-256-gcm/auto)"></div>
          </div>
          <button class="btn btn-success btn-sm" onclick="addNode()">Add</button>
        </div>
        <div class="card">
          <div class="rows" id="node-list"><div class="hint">Loading…</div></div>
        </div>
      </div>

      <!-- ── Page: Subscriptions ───────────────────────────────────── -->
      <div class="page" id="page-subs">
        <div class="page-title">Subscriptions</div>
        <div class="card">
          <h3>Add subscription</h3>
          <div class="form-grid" style="grid-template-columns: 1fr 2fr 110px auto">
            <div class="fg"><label>Name</label><input type="text" id="sub-name" placeholder="Sub 1"></div>
            <div class="fg"><label>URL</label><input type="url" id="sub-url" placeholder="https://..."></div>
            <div class="fg"><label>Format</label>
              <select id="sub-format">
                <option value="auto" selected>auto</option>
                <option value="clash">clash</option>
                <option value="v2ray">v2ray (URI / base64)</option>
                <option value="singbox">sing-box</option>
                <option value="base64">base64</option>
              </select>
            </div>
            <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" onclick="addSub()">Add</button></div>
          </div>
        </div>
        <div class="card">
          <div class="toolbar">
            <h3 style="margin:0">Subscription list</h3>
            <div class="spacer"></div>
            <button class="btn btn-accent btn-sm" onclick="updateSubs()">Update all subscriptions</button>
          </div>
          <div class="rows" id="sub-list"><div class="hint">Loading…</div></div>
          <div class="hint">Subscription updates run in the background; the node list refreshes automatically when done.</div>
        </div>
      </div>

      <!-- ── Page: Routing rules ─────────────────────────────────────── -->
      <div class="page" id="page-dns">
        <div class="page-title">Routing rules</div>
        <div class="card">
          <h3>Add rule</h3>
          <div class="form-grid" style="grid-template-columns: 150px 1fr 110px auto">
            <div class="fg"><label>Type</label>
              <select id="rule-type" onchange="ruleTypeChanged()">
                <option value="domain-suffix">Domain suffix</option>
                <option value="domain">Domain (exact)</option>
                <option value="domain-keyword">Domain keyword</option>
                <option value="ip-cidr">IP CIDR</option>
                <option value="geoip">GeoIP country</option>
                <option value="final">Final (match all)</option>
              </select>
            </div>
            <div class="fg"><label>Pattern</label><input type="text" id="rule-pattern" placeholder="example.com"></div>
            <div class="fg"><label>Action</label>
              <select id="rule-target">
                <option value="direct">Direct</option>
                <option value="proxy">Proxy</option>
                <option value="reject">Reject</option>
              </select>
            </div>
            <div class="fg"><label>&nbsp;</label><button class="btn btn-success btn-sm" onclick="addRule()">Add</button></div>
          </div>
          <div class="hint">Rules are matched top-down, first match wins; new rules are inserted at the top. Changes take effect after an automatic service restart. China domains are direct by default (decided by DNS split) — no rule needed.</div>
        </div>
        <div class="card">
          <h3>Rule list</h3>
          <div class="rows" id="rule-list"><div class="hint">Loading…</div></div>
        </div>
      </div>

      <!-- ── Page: Config ──────────────────────────────────────────── -->
      <div class="page" id="page-config">
        <div class="page-title">Config</div>
        <div class="toolbar">
          <button class="btn btn-success btn-sm" onclick="saveConfig()">Save & validate</button>
          <button class="btn btn-sm" onclick="loadConfig()">Reload</button>
          <span class="hint">Saving runs sockrocket-cli validate automatically; a failed validation never overwrites the current config</span>
        </div>
        <textarea id="config-text" spellcheck="false"></textarea>
      </div>

      <!-- ── Page: Logs ────────────────────────────────────────────── -->
      <div class="page" id="page-logs">
        <div class="page-title">Logs</div>
        <div class="toolbar">
          <button class="btn btn-sm" onclick="loadLog()">Refresh</button>
          <button class="btn btn-danger btn-sm" onclick="clearLog()">Clear</button>
          <label class="hint" style="display:flex;align-items:center;gap:4px;cursor:pointer">
            <input type="checkbox" id="log-auto" onchange="toggleLogAuto()"> Auto-refresh (3s)
          </label>
        </div>
        <div id="log-view">Loading…</div>
      </div>
    </div>
  </div>

  <!-- ── Status bar ────────────────────────────────────────────────── -->
  <div id="statusbar">
    <div class="sb-left">
      <span id="sb-dot">●</span>
      <span id="sb-state">Stopped</span>
      <span id="sb-node" style="overflow:hidden;text-overflow:ellipsis;max-width:280px"></span>
    </div>
    <div class="sb-right">
      <span id="sb-ports">SOCKS :1080 · HTTP :1087</span>
      <span id="sb-uptime"></span>
      <span id="sb-nodes">0 nodes</span>
      <span id="sb-ver"></span>
    </div>
  </div>
</div>
<div id="msg"></div>

<script>
/* ══ API ═══════════════════════════════════════════════════════════ */
/* Three transports, auto-detected at boot:
 *   'http' — sockrocket-cli's built-in API server (http://<router>:18188/api),
 *            always-on, works whether or not the proxy daemon is running
 *   'cgi'  — legacy: direct CGI at /cgi-bin/sockrocket.cgi (the stock Merlin
 *            httpd does NOT execute user CGIs; kept for old installs)
 *   'ksc'  — koolcenter software center (read-only /www): POST /_api/
 *            dispatches sockrocket_api.sh, result polled from /_temp/*.json */
var TRANSPORT = null;
/* Port comes from api.js (generated by sockrocket.sh from config.yaml's api_port);
 * 18188 is the built-in default matching sockrocket-cli's DEFAULT_API_PORT. */
var API_PORT = (typeof SOCKROCKET_API_PORT === 'number' && SOCKROCKET_API_PORT > 0) ? SOCKROCKET_API_PORT : 18188;

function apiBase() { return 'http://' + location.hostname + ':' + API_PORT + '/api'; }

function probeTransport(cb) {
  probeGet(apiBase() + '?action=version', 2500, function (ok) {
    if (ok) { TRANSPORT = 'http'; return cb(); }
    probeGet('/cgi-bin/sockrocket.cgi?action=version', 0, function (ok2) {
      TRANSPORT = ok2 ? 'cgi' : 'ksc';
      cb();
    });
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
    if (timeout) xhr.timeout = timeout;
    xhr.onload = function () { finish(xhr.status === 200 && xhr.responseText.indexOf('"version"') >= 0); };
    xhr.onerror = function () { finish(false); };
    xhr.ontimeout = function () { finish(false); };
    xhr.send(null);
  } catch (e) { finish(false); }
}

function api(action, data, cb, errCb, timeoutMs) {
  if (TRANSPORT === 'ksc') return apiKsc(action, data, cb, errCb);
  return apiXhr(TRANSPORT === 'http' ? apiBase() : '/cgi-bin/sockrocket.cgi', action, data, cb, errCb, timeoutMs);
}

function apiXhr(base, action, data, cb, errCb, timeoutMs) {
  var xhr = new XMLHttpRequest();
  // Same as probeGet: open()/setRequestHeader() throw on a malformed URL and
  // must not escape uncaught.
  try {
    xhr.open(data ? 'POST' : 'GET', base + '?action=' + action, true);
    xhr.setRequestHeader('Content-Type', 'application/json');
    if (timeoutMs) xhr.timeout = timeoutMs;
  } catch (e) {
    showMsg('Cannot connect to the Sockrocket API service', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
    return;
  }
  xhr.onload = function () {
    try {
      var d = JSON.parse(xhr.responseText);
      if (d.error_code || d.error) {
        showMsg('Error: ' + (d.error_message || d.error || d.error_code), 'err');
        if (errCb) errCb(d);
      } else { cb(d); }
    } catch (e) {
      showMsg('Failed to parse API response', 'err');
      if (errCb) errCb({ error_code: 'parse_error' });
    }
  };
  xhr.onerror = function () {
    showMsg('Cannot connect to the Sockrocket API service', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
  };
  xhr.ontimeout = function () {
    if (errCb) errCb({ error_code: 'timeout' });
  };
  xhr.send(data ? JSON.stringify(data) : null);
}

function apiKsc(action, data, cb, errCb) {
  // Full ms timestamp + wide random suffix: the old `Date.now() % 1e9 +
  // random(0..999)` form collided when two requests fired within the same
  // millisecond, making them poll (and possibly read) the same result file.
  var seq = String(Date.now()) + String(Math.floor(Math.random() * 1000000));
  var job = {
    id: seq,
    method: 'sockrocket_api.sh',
    params: [action],
    fields: { sockrocket_rpc_seq: seq, sockrocket_rpc_args: data ? JSON.stringify(data) : '' }
  };
  var xhr = new XMLHttpRequest();
  xhr.open('POST', '/_api/', true);
  xhr.setRequestHeader('Content-Type', 'application/json');
  xhr.onload = function () { pollKscResult(seq, 0, cb, errCb); };
  xhr.onerror = function () {
    showMsg('Cannot connect to the router API', 'err');
    if (errCb) errCb({ error_code: 'network_error' });
  };
  xhr.send(JSON.stringify(job));
}

function pollKscResult(seq, tries, cb, errCb) {
  if (tries > 250) {
    showMsg('Request timed out', 'err');
    if (errCb) errCb({ error_code: 'timeout' });
    return;
  }
  var xhr = new XMLHttpRequest();
  xhr.open('GET', '/_temp/sockrocket_rpc_' + seq + '.json?_=' + Math.random(), true);
  xhr.onload = function () {
    var t = (xhr.responseText || '').trim();
    if (xhr.status === 200 && t.charAt(0) === '{') {
      try {
        var d = JSON.parse(t);
        if (d.error_code || d.error) {
          showMsg('Error: ' + (d.error_message || d.error || d.error_code), 'err');
          if (errCb) errCb(d);
        } else { cb(d); }
        return;
      } catch (e) { /* fall through to retry */ }
    }
    setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb); }, 200);
  };
  xhr.onerror = function () {
    setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb); }, 200);
  };
  try { xhr.send(null); } catch (e) { setTimeout(function () { pollKscResult(seq, tries + 1, cb, errCb); }, 200); }
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
function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
    return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
  });
}
function latClass(ms) { return ms < 0 ? 'bad' : ms < 200 ? 'good' : ms < 500 ? 'mid' : 'bad'; }
function latText(ms) { return ms < 0 ? 'timeout' : ms + 'ms'; }

var msgTimer;
function showMsg(msg, type) {
  clearTimeout(msgTimer);
  var el = document.getElementById('msg');
  el.textContent = msg;
  el.className = type || 'info';
  el.style.display = 'block';
  msgTimer = setTimeout(function () { el.style.display = 'none'; }, type === 'err' ? 8000 : 4000);
}
function toastResp(d, fallback) {
  if (d.ok === false) showMsg(d.msg || 'Operation failed', 'err');
  else showMsg(d.msg || fallback || 'Done', 'ok');
}

/* ══ State ═════════════════════════════════════════════════════════ */
var S = {
  page: 'home',
  status: {},        // status action result
  stats: {},         // stats action result
  nodes: [],         // node_details
  activeNode: 0,
  lat: {},           // index -> ms (-1 = timeout)
  testingAll: false,
};
var PAGES = ['home', 'nodes', 'subs', 'dns', 'config', 'logs'];
var PAGE_LOADED = {};

function showPage(p) {
  S.page = p;
  PAGES.forEach(function (id) {
    document.getElementById('page-' + id).classList.toggle('active', id === p);
    document.getElementById('nav-' + id).classList.toggle('active', id === p);
  });
  if (!PAGE_LOADED[p]) {
    PAGE_LOADED[p] = true;
    if (p === 'nodes') loadNodes();
    if (p === 'subs') loadSubs();
    if (p === 'dns') loadRules();
    if (p === 'config') loadConfig();
    if (p === 'logs') loadLog();
  } else {
    if (p === 'nodes') renderNodes();
  }
}

/* ══ Title bar / status bar / home ═════════════════════════════════ */
function refreshStatus(withSysinfo) {
  api('status', null, function (d) {
    S.status = d;
    renderChrome();
  });
  api('stats', null, function (d) {
    S.stats = d;
    renderChrome();
  });
  // sysinfo (mem/load/wan_ip) changes slowly; every CGI call forks a
  // sockrocket-cli process on the router, so poll it at 1/5 the rate.
  if (withSysinfo) api('sysinfo', null, function (d) {
    S.sysinfo = d;
    renderChrome();
  });
}

function renderChrome() {
  var st = S.status, ss = S.stats, si = S.sysinfo || {};
  var running = !!st.running;

  // title bar
  var nbName = document.getElementById('nb-name');
  nbName.textContent = S.nodes[st.active_node] ? S.nodes[st.active_node].name : (st.active_node != null ? 'Node #' + st.active_node : '—');
  var activeLat = S.lat[st.active_node];
  var nbLat = document.getElementById('nb-lat');
  nbLat.textContent = activeLat != null ? latText(activeLat) : '';
  nbLat.className = 'nb-lat' + (activeLat != null && activeLat < 0 ? ' bad' : '');

  // status bar
  var dot = document.getElementById('sb-dot');
  dot.style.color = running ? 'var(--success)' : 'var(--text-muted)';
  document.getElementById('sb-state').textContent = running ? 'Running' : 'Stopped';
  document.getElementById('sb-state').className = running ? 'ok' : '';
  document.getElementById('sb-node').textContent =
    (S.nodes[st.active_node] ? S.nodes[st.active_node].name : '') +
    (activeLat != null ? ' · ' + latText(activeLat) : '');
  document.getElementById('sb-ports').textContent = 'SOCKS :' + (st.socks_port || 1080) + ' · HTTP :' + (st.http_port || 1087);
  document.getElementById('sb-uptime').textContent = ss.uptime && ss.uptime !== 'stopped' ? '↑ ' + ss.uptime : '';
  document.getElementById('sb-nodes').textContent = (st.node_count || 0) + ' nodes';
  document.getElementById('sb-ver').textContent = st.version || '';

  // home hero
  var hi = document.getElementById('hero-icon');
  hi.className = 'hero-icon' + (running ? '' : ' off');
  document.getElementById('hero-state').textContent = running ? 'Connected' : 'Stopped';
  document.getElementById('hero-state').style.color = running ? 'var(--success)' : 'var(--text-secondary)';
  document.getElementById('hero-live').style.display = running ? '' : 'none';
  var n = S.nodes[st.active_node];
  document.getElementById('hero-node').textContent = n
    ? n.name + ' · ' + (n.protocol || '?') + ' · ' + (n.port || '?')
    : (st.node_count > 0 ? 'Node #' + st.active_node : 'No nodes — add a subscription or node first');
  document.getElementById('hero-uptime').textContent = running ? (ss.uptime || '—') : '—';
  // health check line: probe latency / failure streak / failover history
  var hh = document.getElementById('hero-health');
  var health = st.health;
  if (running && health && health.current_node_name) {
    var hp = ['Heartbeat ' + (health.last_probe_ms != null ? health.last_probe_ms + 'ms' : '—')];
    if (health.consecutive_failures > 0) hp.push(health.consecutive_failures + ' consecutive failures');
    if (health.switch_count > 0 && health.last_switch) {
      var swd = new Date(health.last_switch.at * 1000);
      var swt = ('0' + swd.getHours()).slice(-2) + ':' + ('0' + swd.getMinutes()).slice(-2);
      hp.push('Auto-switched ' + health.switch_count + ' time(s) (latest → ' + health.last_switch.node_name + ' ' + swt + ')');
    } else {
      hp.push('No switches yet');
    }
    hh.textContent = '⚡ ' + hp.join(' · ');
    hh.style.color = health.consecutive_failures > 0 ? 'var(--warning, orange)' : 'var(--text-muted)';
    hh.style.display = '';
  } else {
    hh.style.display = 'none';
  }
  document.getElementById('hero-nodes').textContent = st.node_count || 0;
  document.getElementById('hero-subs').textContent = st.sub_count || 0;
  var tb = document.getElementById('toggle-btn');
  tb.textContent = running ? 'Stop service' : 'Start service';
  tb.className = 'btn ' + (running ? 'btn-danger' : 'btn-success');

  // chips: the config keys are the source of truth (they survive a service
  // stop), unlike the runtime stats which go inactive with the daemon.
  setChip('dns', st.dns_hijack);
  setChip('fw', st.transparent_proxy);
  setChip('cn', st.cn_ipset_direct);
  document.getElementById('wan-ip').textContent = si.wan_ip || '—';

  // stat cards
  document.getElementById('st-mem').textContent = si.mem_pct != null ? si.mem_pct + '%' : '—';
  document.getElementById('st-mem-sub').textContent = si.mem_used_kb ? Math.round(si.mem_used_kb / 1024) + ' / ' + Math.round(si.mem_total_kb / 1024) + ' MB' : '';
  document.getElementById('st-load').textContent = si.load || '—';
  document.getElementById('st-uptime').textContent = si.uptime ? 'Router uptime ' + si.uptime : '';
}

function setChip(id, on) {
  var dot = document.getElementById(id + '-dot');
  if (!dot) return;
  dot.className = 'dot ' + (on ? 'ok' : 'off');
  document.getElementById(id + '-state').textContent = on ? 'On' : 'Off';
}

/* Chips are switches now: the API call flips the key, then the next status
 * poll renders the result. While in flight the chip is disabled — rapid
 * clicking used to fire overlapping service restarts. */
function chipPending(id, on) {
  var el = document.getElementById(id);
  if (el) el.classList.toggle('pending', on);
}
function toggleDns() {
  var want = !S.status.dns_hijack;
  chipPending('chip-dns', true);
  api('set_toggles', { dns_hijack: want }, function (d) {
    chipPending('chip-dns', false);
    toastResp(d);
    refreshStatus(true);
  }, function () { chipPending('chip-dns', false); });
}
function toggleCn() {
  var want = !S.status.cn_ipset_direct;
  chipPending('chip-cn', true);
  api('set_toggles', { cn_ipset_direct: want }, function (d) {
    chipPending('chip-cn', false);
    toastResp(d);
    refreshStatus(true);
  }, function () { chipPending('chip-cn', false); });
}
function toggleProxy() {
  var want = !S.status.transparent_proxy;
  chipPending('chip-fw', true);
  api('set_toggles', { transparent_proxy: want }, function (d) {
    chipPending('chip-fw', false);
    toastResp(d);
    // Turning the proxy on restarts the service (~10s before the TUN device
    // and iptables rules are in place), so give it room before polling.
    setTimeout(function () { refreshStatus(true); }, 1500);
  }, function () { chipPending('chip-fw', false); });
}

/* ══ Service control ═══════════════════════════════════════════════ */
function toggleService() {
  var action = S.status.running ? 'stop' : 'start';
  api(action, {}, function (d) {
    toastResp(d);
    setTimeout(refreshStatus, 800);
  });
}
function restartService() {
  api('restart', {}, function (d) {
    toastResp(d);
    setTimeout(refreshStatus, 800);
  });
}

/* ══ Speedtest / IP check / diagnose ═══════════════════════════════ */
function runSpeedtest() {
  document.getElementById('st-lat').textContent = '…';
  document.getElementById('st-lat-sub').textContent = 'Testing…';
  api('speedtest', null, function (d) {
    var el = document.getElementById('st-lat');
    if (d.ok && d.latency >= 0) {
      el.textContent = d.latency + 'ms';
      el.className = 'value ' + (d.latency < 200 ? 'ok' : d.latency < 500 ? 'acc' : 'bad');
      document.getElementById('st-lat-sub').textContent = 'via current node';
      S.lat[S.status.active_node] = d.latency;
      renderChrome();
    } else {
      el.textContent = 'Failed';
      el.className = 'value bad';
      document.getElementById('st-lat-sub').textContent = d.msg || 'Connection failed';
    }
  });
}
function runIpCheck() {
  document.getElementById('st-ip').textContent = '…';
  document.getElementById('st-ip-sub').textContent = 'Checking…';
  api('ip_check', null, function (d) {
    if (d.ok && d.ip) {
      document.getElementById('st-ip').textContent = d.ip;
      document.getElementById('st-ip-sub').textContent = d.country || '';
    } else {
      document.getElementById('st-ip').textContent = 'Failed';
      document.getElementById('st-ip-sub').textContent = d.msg || '';
    }
  });
}
function toggleDiag() {
  var card = document.getElementById('diag-card');
  if (card.classList.contains('open')) { card.classList.remove('open'); return; }
  card.classList.add('open');
  document.getElementById('diag-body').innerHTML = '<div class="hint">Diagnosing…</div>';
  api('diagnose', null, function (d) {
    function row(name, ok, detail) {
      return '<div class="diag-row"><span class="d-name">' + name + '</span>' +
        '<span class="' + (ok ? 'check-ok' : 'check-bad') + '">' + (ok ? '✓ ' : '✗ ') + esc(detail) + '</span></div>';
    }
    var html =
      row('sockrocket-cli process', d.running, d.running ? 'Running' : 'Not running') +
      row('Config validation', d.config_valid, d.config_valid ? 'Passed' : 'Failed') +
      row('Transparent proxy rules', d.iptables_rules > 0, d.iptables_rules + ' MARK rule(s)') +
      row('DNS takeover', d.dns_rules > 0, d.dns_rules > 0 ? 'Enabled' : 'Disabled (DNS handled by the TUN stack)') +
      row('dnsmasq', d.dnsmasq_running, d.dnsmasq_running ? 'Running' : 'Not running') +
      row('Firewall chain', d.fw_active, d.fw_active ? 'Mounted' : 'Not mounted');
    var errs = b64utf8(d.last_errors_base64).trim();
    if (errs) html += '<h3 style="margin-top:10px">Recent errors</h3><div class="log-block" style="height:auto;max-height:160px">' + esc(errs) + '</div>';
    document.getElementById('diag-body').innerHTML = html;
  });
}

/* ══ Nodes ═════════════════════════════════════════════════════════ */
function loadNodes() {
  api('node_details', null, function (d) {
    S.nodes = d.nodes || [];
    S.activeNode = d.active || 0;
    renderNodes();
    renderChrome();
  });
}
function renderNodes() {
  var el = document.getElementById('node-list');
  document.getElementById('nodes-count').textContent = S.nodes.length + ' node(s)';
  if (!S.nodes.length) { el.innerHTML = '<div class="hint">No nodes yet — add a subscription or a node manually.</div>'; return; }
  var html = '';
  var activeIdx = Number(S.status.active_node);
  S.nodes.forEach(function (n, i) {
    var lat = S.lat[i];
    html += '<div class="row' + (i === activeIdx ? ' active' : '') + '" onclick="setNode(' + i + ')">' +
      '<span class="badge">' + esc(n.protocol || '?') + '</span>' +
      '<span class="r-name">' + esc(n.name) + '</span>' +
      '<span class="r-sub">' + esc(n.server) + ':' + esc(n.port) + '</span>' +
      '<span class="lat ' + (lat != null ? latClass(lat) : '') + '" id="lat-' + i + '">' + (lat != null ? latText(lat) : '—') + '</span>' +
      '<button class="btn btn-sm" onclick="event.stopPropagation();testNode(' + i + ')">Test</button>' +
      '<button class="btn btn-danger btn-sm" onclick="event.stopPropagation();delNode(' + i + ')">Delete</button>' +
      '</div>';
  });
  el.innerHTML = html;
}
function setNode(i) {
  api('set_node', { index: i }, function (d) {
    toastResp(d, 'Switched to node #' + i);
    S.status.active_node = i;
    renderNodes();
    renderChrome();
    // Re-probe with the same warm through-proxy path as every other node
    // (not SOCKS-only), after ConfigWatcher has swapped the outbound.
    setTimeout(function () { testNode(i); }, 2500);
  });
}
function testNode(i) {
  var el = document.getElementById('lat-' + i);
  if (el) el.textContent = '…';
  // Warm through-proxy probe (warmup + measure). Cap wait for Reality/QUIC.
  api('speedtest_node', { index: i }, function (d) {
    S.lat[i] = d.ok && d.latency >= 0 ? d.latency : -1;
    renderNodes();
    renderChrome();
  }, function () {
    S.lat[i] = -1;
    renderNodes();
    renderChrome();
  }, 60000);
}
function testAllNodes() {
  if (S.testingAll) return;
  S.testingAll = true;
  var btn = document.getElementById('test-all-btn');
  btn.disabled = true; btn.textContent = 'Testing…';
  // Keep concurrency low on Merlin: each probe is a full protocol handshake
  // and the API also caps at 2 in-process. Higher parallelism starved the
  // router CPU and produced mass false timeouts.
  var CONCURRENCY = 2;
  var next = 0;
  var inflight = 0;
  function finishAll() {
    S.testingAll = false;
    btn.disabled = false;
    btn.textContent = 'Test all';
  }
  function pump() {
    if (S.page !== 'nodes') { finishAll(); return; }
    while (inflight < CONCURRENCY && next < S.nodes.length) {
      (function (idx) {
        inflight++;
        var el = document.getElementById('lat-' + idx);
        if (el) el.textContent = '…';
        function done(ok, d) {
          S.lat[idx] = ok && d && d.ok && d.latency >= 0 ? d.latency : -1;
          renderNodes();
          inflight--;
          if (next >= S.nodes.length && inflight === 0) finishAll();
          else pump();
        }
        api('speedtest_node', { index: idx },
          function (d) { done(true, d); },
          function () { done(false, null); },
          60000);
      })(next++);
    }
  }
  pump();
}
function addNode() {
  var data = {
    name: document.getElementById('an-name').value.trim(),
    protocol: document.getElementById('an-proto').value,
    server: document.getElementById('an-server').value.trim(),
    port: parseInt(document.getElementById('an-port').value, 10) || 0,
    password: document.getElementById('an-pass').value,
    uuid: document.getElementById('an-uuid').value.trim(),
    cipher: document.getElementById('an-cipher').value.trim(),
  };
  if (!data.name || !data.server || !data.port) { showMsg('Name, server and port must not be empty', 'err'); return; }
  api('add_node', data, function (d) {
    toastResp(d);
    if (d.ok !== false) { loadNodes(); refreshStatus(); }
  });
}
function delNode(i) {
  if (!confirm('Delete node ' + (S.nodes[i] ? S.nodes[i].name : '#' + i) + ' ?')) return;
  api('del_node', { index: i }, function (d) {
    toastResp(d);
    if (d.ok !== false) { loadNodes(); refreshStatus(); }
  });
}

/* ══ Subscriptions ═════════════════════════════════════════════════ */
function loadSubs() {
  api('subscriptions', null, function (d) {
    var el = document.getElementById('sub-list');
    var subs = d.subs || [];
    if (!subs.length) { el.innerHTML = '<div class="hint">No subscriptions yet.</div>'; return; }
    el.innerHTML = subs.map(function (s) {
      return '<div class="row">' +
        '<span class="badge acc">' + esc(s.format || 'auto') + '</span>' +
        '<span class="r-name">' + esc(s.name) + '<div class="r-sub">' + esc(s.url) + '</div></span>' +
        '<button class="btn btn-danger btn-sm" onclick="event.stopPropagation();delSub(' + s.index + ')">Delete</button>' +
        '</div>';
    }).join('');
  });
}
function addSub() {
  var data = {
    name: document.getElementById('sub-name').value.trim(),
    url: document.getElementById('sub-url').value.trim(),
    format: document.getElementById('sub-format').value,
  };
  if (!data.name || !data.url) { showMsg('Name and URL must not be empty', 'err'); return; }
  showMsg('Fetching subscription…', 'info');
  api('add_sub', data, function (d) {
    toastResp(d);
    loadSubs();
    if (d.ok !== false) {
      document.getElementById('sub-name').value = '';
      document.getElementById('sub-url').value = '';
      // Nodes are persisted before restart; refresh soon, then again after restart
      setTimeout(function () { loadNodes(); refreshStatus(); }, 1500);
      setTimeout(function () { loadNodes(); refreshStatus(); }, 8000);
    }
  }, function () {
    showMsg('Subscription request failed (timeout or network)', 'err');
  });
}
function delSub(i) {
  if (!confirm('Delete subscription #' + i + ' ?')) return;
  api('del_sub', { index: i }, function (d) {
    toastResp(d);
    if (d.ok !== false) { loadSubs(); refreshStatus(); }
  });
}
function updateSubs() {
  showMsg('Updating subscriptions…', 'info');
  api('update_subs', {}, function (d) {
    toastResp(d);
    setTimeout(function () { loadNodes(); refreshStatus(); }, 1500);
    setTimeout(function () { loadNodes(); refreshStatus(); }, 8000);
  }, function () {
    showMsg('Update request failed (timeout or network)', 'err');
  });
}

/* ══ Routing rules ═════════════════════════════════════════════════ */
var RULE_TYPE_LABEL = { 'domain': 'Domain', 'domain-suffix': 'Suffix', 'domain-keyword': 'Keyword', 'ip-cidr': 'IP CIDR', 'geoip': 'GeoIP', 'final': 'Final', 'match': 'Final' };
var RULE_TARGET_LABEL = { 'direct': 'Direct', 'proxy': 'Proxy', 'reject': 'Reject' };
var RULE_PATTERN_HINT = {
  'domain': 'www.example.com', 'domain-suffix': 'example.com', 'domain-keyword': 'google',
  'ip-cidr': '10.0.0.0/8', 'geoip': 'CN', 'final': '(not required)'
};
function ruleTypeChanged() {
  var t = document.getElementById('rule-type').value;
  var p = document.getElementById('rule-pattern');
  p.placeholder = RULE_PATTERN_HINT[t] || '';
  p.disabled = (t === 'final');
  if (t === 'final') p.value = '';
}
function loadRules() {
  api('get_rules', null, function (d) {
    var el = document.getElementById('rule-list');
    var rules = d.rules || [];
    if (!rules.length) { el.innerHTML = '<div class="hint">No rules yet (default: private and CN traffic direct, everything else proxied).</div>'; return; }
    el.innerHTML = rules.map(function (r) {
      return '<div class="row"><span class="r-name" style="font-family:var(--mono)">' +
        esc(RULE_TYPE_LABEL[r.rule_type] || r.rule_type) + '　' + esc(r.pattern) +
        '　→ ' + esc(RULE_TARGET_LABEL[r.target] || r.target) + '</span>' +
        '<button class="btn btn-danger btn-sm" onclick="event.stopPropagation();delRule(' + r.index + ')">Delete</button></div>';
    }).join('');
  });
}
function addRule() {
  var t = document.getElementById('rule-type').value;
  var p = document.getElementById('rule-pattern').value.trim();
  var g = document.getElementById('rule-target').value;
  if (t !== 'final' && !p) { showMsg('Please enter a pattern', 'err'); return; }
  api('add_rule', { rule_type: t, pattern: p, target: g }, function (d) {
    toastResp(d);
    if (d.ok !== false) { document.getElementById('rule-pattern').value = ''; loadRules(); }
  });
}
function delRule(i) {
  api('del_rule', { index: i }, function (d) {
    toastResp(d);
    if (d.ok !== false) loadRules();
  });
}

/* ══ Config ════════════════════════════════════════════════════════ */
function loadConfig() {
  api('config', null, function (d) {
    document.getElementById('config-text').value = b64utf8(d.config_base64);
  });
}
function saveConfig() {
  var text = document.getElementById('config-text').value;
  api('save_config', { config: utf8b64(text) }, function (d) {
    toastResp(d);
    setTimeout(refreshStatus, 800);
  });
}

/* ══ Logs ══════════════════════════════════════════════════════════ */
var logAutoTimer = null;
function loadLog() {
  api('log', null, function (d) {
    var el = document.getElementById('log-view');
    var atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 30;
    el.textContent = b64utf8(d.log_base64) || '(log is empty)';
    if (atBottom) el.scrollTop = el.scrollHeight;
  });
}
function clearLog() {
  api('logs_clear', {}, function (d) {
    toastResp(d);
    loadLog();
  });
}
function toggleLogAuto() {
  var on = document.getElementById('log-auto').checked;
  clearInterval(logAutoTimer);
  if (on) logAutoTimer = setInterval(function () { if (S.page === 'logs') loadLog(); }, 3000);
}

/* ══ Boot ══════════════════════════════════════════════════════════ */
probeTransport(function () {
  loadNodes();           // needed by title bar node badge + home hero
  refreshStatus(true);
  var tick = 0;
  setInterval(function () { refreshStatus(++tick % 5 === 0); }, 4000);
});
</script>
</body>
</html>
