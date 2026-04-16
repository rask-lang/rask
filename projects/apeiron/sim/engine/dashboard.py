"""Live web dashboard for sim visualization.

Starts a tiny HTTP server in a background thread. Serves one HTML page.
Pushes tick updates via Server-Sent Events. Receives commands via POST.
No dependencies beyond stdlib + Chart.js from CDN.

Usage:
    dashboard = Dashboard(port=8050)
    world = dashboard.run(world, ticks=1000)  # replaces engine.run()
"""

from __future__ import annotations

import json
import threading
import time
import webbrowser
from http.server import HTTPServer, BaseHTTPRequestHandler
from typing import Any

from .world import World, step


class Dashboard:
    def __init__(self, port: int = 8050):
        self.port = port
        self.speed = 10  # ticks per second (0 = max speed)
        self.paused = False
        self.step_one = False
        self.snapshot: dict[str, Any] = {}
        self.galaxy_data: list[dict] | None = None
        self.tick_version = 0
        self._server = None

    def run(self, world: World, ticks: int) -> World:
        self._build_galaxy_data(world)
        self._start_server()
        webbrowser.open(f"http://localhost:{self.port}")

        for _ in range(ticks):
            while self.paused and not self.step_one:
                time.sleep(0.02)
            self.step_one = False

            world = step(world)
            self._push_snapshot(world)

            if self.speed > 0 and not self.paused:
                time.sleep(1.0 / self.speed)

        return world

    def _build_galaxy_data(self, world: World):
        locations = []
        for loc in world.locations.values():
            pos = loc.position
            deposits = loc.state.get("deposits", {})
            locations.append({
                "id": loc.id,
                "x": pos[0] if len(pos) > 0 else 0,
                "y": pos[1] if len(pos) > 1 else 0,
                "z": pos[2] if len(pos) > 2 else 0,
                "neighbors": loc.neighbors,
                "elements": list(deposits.keys()),
                "total_deposits": sum(deposits.values()),
            })
        self.galaxy_data = locations

    def _push_snapshot(self, world: World):
        agents = []
        for a in world.agents.values():
            agents.append({
                "id": a.id,
                "location": a.state.location,
                "credits": round(a.state.credits, 1),
                "inventory": {k: round(v, 1) for k, v in a.state.inventory.items()
                              if v > 0.1},
            })

        metrics = {}
        for key in ["total_credits", "total_fuel"]:
            val = world.recorder.last(key)
            if val is not None:
                metrics[key] = round(val, 1)

        trade_keys = [k for k in world.recorder.keys()
                      if k.startswith("trade.") and k.endswith(".qty")]
        for k in trade_keys:
            s = world.recorder.series(k)
            if s:
                last_tick_trades = [v for t, v in s if t == world.tick - 1]
                if last_tick_trades:
                    metrics[k] = round(sum(last_tick_trades), 1)

        recent_entries = [(t, k, v) for t, k, v in world.recorder.entries
                          if t >= world.tick - 2]
        events = []
        for t, k, v in recent_entries[-20:]:
            if "trade." in k or "fuel_burned" in k or "extracted." in k:
                events.append({"tick": t, "key": k, "value": round(v, 2)})

        # Time series: last 200 ticks
        series = {}
        for key in ["total_credits", "total_fuel"]:
            s = world.recorder.series(key)
            if s:
                series[key] = s[-200:]

        self.snapshot = {
            "tick": world.tick,
            "agents": agents,
            "metrics": metrics,
            "events": events,
            "series": {k: [[t, v] for t, v in pts] for k, pts in series.items()},
            "speed": self.speed,
            "paused": self.paused,
        }
        self.tick_version += 1

    def _start_server(self):
        dashboard = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/":
                    self.send_response(200)
                    self.send_header("Content-Type", "text/html")
                    self.end_headers()
                    self.wfile.write(_HTML.encode())
                elif self.path == "/events":
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Cache-Control", "no-cache")
                    self.send_header("Access-Control-Allow-Origin", "*")
                    self.end_headers()

                    # Send galaxy data first
                    if dashboard.galaxy_data:
                        msg = json.dumps({"type": "galaxy",
                                          "locations": dashboard.galaxy_data})
                        self.wfile.write(f"data: {msg}\n\n".encode())
                        self.wfile.flush()

                    last_ver = -1
                    try:
                        while True:
                            if dashboard.tick_version > last_ver:
                                msg = json.dumps({"type": "tick",
                                                  **dashboard.snapshot})
                                self.wfile.write(f"data: {msg}\n\n".encode())
                                self.wfile.flush()
                                last_ver = dashboard.tick_version
                            time.sleep(0.04)
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                else:
                    self.send_error(404)

            def do_POST(self):
                if self.path == "/command":
                    length = int(self.headers.get("Content-Length", 0))
                    body = self.rfile.read(length)
                    cmd = json.loads(body)
                    if "speed" in cmd:
                        dashboard.speed = max(0, cmd["speed"])
                    if "paused" in cmd:
                        dashboard.paused = bool(cmd["paused"])
                    if cmd.get("step"):
                        dashboard.step_one = True
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.end_headers()
                    self.wfile.write(b'{"ok":true}')
                else:
                    self.send_error(404)

            def log_message(self, format, *args):
                pass  # silence request logs

        server = HTTPServer(("127.0.0.1", self.port), Handler)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self._server = server


_HTML = r"""<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Apeiron Sim Dashboard</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4"></script>
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
    background: #080810; color: #aab;
    font-family: 'JetBrains Mono', 'Fira Code', 'SF Mono', monospace;
    font-size: 13px; overflow: hidden; height: 100vh;
}
.grid {
    display: grid; height: 100vh;
    grid-template-columns: 1fr 320px;
    grid-template-rows: 48px 1fr 1fr;
    gap: 2px;
}
.header {
    grid-column: 1 / -1; background: #0c0c1a;
    display: flex; align-items: center; padding: 0 16px; gap: 24px;
    border-bottom: 1px solid #1a1a2e;
}
.header h1 { font-size: 15px; color: #0af; font-weight: 600; letter-spacing: 1px; }
.header .tick { color: #fa0; font-size: 14px; }
.header .controls { display: flex; gap: 8px; margin-left: auto; align-items: center; }
.btn {
    background: #1a1a2e; border: 1px solid #2a2a4e; color: #88a;
    padding: 4px 12px; border-radius: 3px; cursor: pointer; font-family: inherit;
    font-size: 12px;
}
.btn:hover { background: #22224a; color: #aac; }
.btn.active { border-color: #0af; color: #0af; }
.speed-display { color: #888; font-size: 12px; min-width: 80px; text-align: center; }
.panel {
    background: #0c0c18; border: 1px solid #161630; border-radius: 2px;
    padding: 10px; overflow: hidden; display: flex; flex-direction: column;
}
.panel-title {
    font-size: 11px; color: #446; text-transform: uppercase;
    letter-spacing: 2px; margin-bottom: 8px; flex-shrink: 0;
}
.galaxy-panel { grid-row: 2 / 4; }
.galaxy-panel canvas { width: 100%; flex: 1; }
.chart-panel canvas { width: 100%; height: 100%; flex: 1; }
.agents-panel { overflow-y: auto; }
.events-panel { overflow-y: auto; }
table { width: 100%; border-collapse: collapse; font-size: 11px; }
th { color: #446; text-align: left; padding: 3px 6px; border-bottom: 1px solid #1a1a2e;
     font-weight: 500; letter-spacing: 1px; text-transform: uppercase; font-size: 10px; }
td { padding: 3px 6px; border-bottom: 1px solid #111125; }
.agent-row .id { color: #0af; }
.agent-row .loc { color: #888; }
.agent-row .credits { color: #fa0; }
.event {
    padding: 2px 0; border-bottom: 1px solid #111120; font-size: 11px;
    display: flex; gap: 8px;
}
.event .t { color: #444; min-width: 48px; }
.event .k { color: #668; }
.event .v { color: #4c8; margin-left: auto; }
.event.trade .k { color: #4c8; }
.event.move .k { color: #08f; }
.event.extract .k { color: #fa0; }
.metrics-bar {
    display: flex; gap: 24px; font-size: 12px;
}
.metric { display: flex; gap: 6px; }
.metric .label { color: #446; }
.metric .value { color: #fa0; font-weight: 600; }
</style>
</head>
<body>
<div class="grid">
    <div class="header">
        <h1>APEIRON</h1>
        <span class="tick" id="tick">TICK 0</span>
        <div class="metrics-bar" id="metrics-bar"></div>
        <div class="controls">
            <button class="btn" id="btn-pause" onclick="togglePause()">PAUSE</button>
            <button class="btn" onclick="stepOne()">STEP</button>
            <button class="btn" onclick="setSpeed(1)">1x</button>
            <button class="btn" onclick="setSpeed(10)">10x</button>
            <button class="btn" onclick="setSpeed(50)">50x</button>
            <button class="btn" onclick="setSpeed(0)">MAX</button>
            <span class="speed-display" id="speed-display">10 t/s</span>
        </div>
    </div>
    <div class="panel galaxy-panel">
        <div class="panel-title">Galaxy</div>
        <canvas id="galaxy"></canvas>
    </div>
    <div class="panel agents-panel">
        <div class="panel-title">Agents</div>
        <table><thead><tr><th>ID</th><th>Loc</th><th>Credits</th><th>Cargo</th></tr></thead>
        <tbody id="agents-body"></tbody></table>
    </div>
    <div class="panel chart-panel">
        <div class="panel-title">Economy</div>
        <canvas id="chart-eco"></canvas>
    </div>
    <div class="panel events-panel">
        <div class="panel-title">Events</div>
        <div id="events-log"></div>
    </div>
</div>
<script>
let galaxyData = null;
let paused = false;
let currentSpeed = 10;

// Charts
const ecoCtx = document.getElementById('chart-eco').getContext('2d');
const ecoChart = new Chart(ecoCtx, {
    type: 'line',
    data: {
        labels: [],
        datasets: [
            { label: 'Credits', data: [], borderColor: '#fa0', borderWidth: 1.5,
              pointRadius: 0, tension: 0.3 },
            { label: 'Fuel', data: [], borderColor: '#0af', borderWidth: 1.5,
              pointRadius: 0, tension: 0.3 },
        ]
    },
    options: {
        responsive: true, maintainAspectRatio: false, animation: false,
        scales: {
            x: { display: true, grid: { color: '#111125' },
                 ticks: { color: '#334', maxTicksLimit: 8 } },
            y: { display: true, grid: { color: '#111125' },
                 ticks: { color: '#334' } }
        },
        plugins: { legend: { labels: { color: '#668', font: { size: 10 } } } }
    }
});

// Galaxy canvas
const galaxyCanvas = document.getElementById('galaxy');
let galaxyCtxG = galaxyCanvas.getContext('2d');
let gBounds = null;
let agentLocations = {};

function drawGalaxy() {
    if (!galaxyData) return;
    const c = galaxyCanvas;
    const ctx = galaxyCtxG;
    c.width = c.clientWidth * devicePixelRatio;
    c.height = c.clientHeight * devicePixelRatio;
    ctx.scale(devicePixelRatio, devicePixelRatio);
    const w = c.clientWidth, h = c.clientHeight;

    ctx.fillStyle = '#060610';
    ctx.fillRect(0, 0, w, h);

    if (!gBounds) {
        let minX=Infinity, maxX=-Infinity, minY=Infinity, maxY=-Infinity;
        for (const loc of galaxyData) {
            minX = Math.min(minX, loc.x); maxX = Math.max(maxX, loc.x);
            minY = Math.min(minY, loc.y); maxY = Math.max(maxY, loc.y);
        }
        const pad = Math.max(maxX-minX, maxY-minY) * 0.1;
        gBounds = { minX: minX-pad, maxX: maxX+pad, minY: minY-pad, maxY: maxY+pad };
    }

    function tx(x) { return (x - gBounds.minX) / (gBounds.maxX - gBounds.minX) * (w - 40) + 20; }
    function ty(y) { return h - ((y - gBounds.minY) / (gBounds.maxY - gBounds.minY) * (h - 40) + 20); }

    // Draw connections
    ctx.strokeStyle = '#1a1a3a';
    ctx.lineWidth = 0.5;
    const locMap = {};
    for (const loc of galaxyData) locMap[loc.id] = loc;
    for (const loc of galaxyData) {
        for (const nid of loc.neighbors) {
            const nb = locMap[nid];
            if (nb && nb.id > loc.id) {
                ctx.beginPath();
                ctx.moveTo(tx(loc.x), ty(loc.y));
                ctx.lineTo(tx(nb.x), ty(nb.y));
                ctx.stroke();
            }
        }
    }

    // Draw active trade routes
    ctx.strokeStyle = '#0af4';
    ctx.lineWidth = 1;
    for (const loc of galaxyData) {
        const agents = Object.entries(agentLocations).filter(([_, l]) => l === loc.id);
        if (agents.length > 0) {
            for (const nid of loc.neighbors) {
                const nb = locMap[nid];
                if (nb) {
                    const nbAgents = Object.entries(agentLocations).filter(([_, l]) => l === nid);
                    if (nbAgents.length > 0) {
                        ctx.beginPath();
                        ctx.moveTo(tx(loc.x), ty(loc.y));
                        ctx.lineTo(tx(nb.x), ty(nb.y));
                        ctx.stroke();
                    }
                }
            }
        }
    }

    // Draw star systems
    for (const loc of galaxyData) {
        const x = tx(loc.x), y = ty(loc.y);
        const r = 4 + Math.min(loc.total_deposits / 200000, 8);

        // Glow
        const grad = ctx.createRadialGradient(x, y, 0, x, y, r * 3);
        grad.addColorStop(0, '#0af3');
        grad.addColorStop(1, '#0af0');
        ctx.fillStyle = grad;
        ctx.beginPath(); ctx.arc(x, y, r * 3, 0, Math.PI * 2); ctx.fill();

        // Core
        ctx.fillStyle = '#0af';
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();

        // Label
        ctx.fillStyle = '#4468';
        ctx.font = '9px monospace';
        ctx.fillText(loc.id.replace('sys_', 'S'), x + r + 3, y + 3);
    }

    // Draw agents as small dots on their locations
    const agentColors = { extractor: '#fa0', station: '#4c8', hauler: '#f4a' };
    for (const [aid, locId] of Object.entries(agentLocations)) {
        const loc = locMap[locId];
        if (!loc) continue;
        const type = aid.split('_')[0];
        const color = agentColors[type] || '#fff';
        const x = tx(loc.x), y = ty(loc.y);
        const offset = (aid.charCodeAt(aid.length-1) % 5 - 2) * 4;
        ctx.fillStyle = color;
        ctx.beginPath(); ctx.arc(x + offset, y - 10 + offset * 0.5, 3, 0, Math.PI * 2);
        ctx.fill();
    }
}

// SSE
const evtSource = new EventSource('/events');
evtSource.onmessage = (e) => {
    const data = JSON.parse(e.data);

    if (data.type === 'galaxy') {
        galaxyData = data.locations;
        drawGalaxy();
        return;
    }

    // Tick update
    document.getElementById('tick').textContent = 'TICK ' + data.tick;

    // Metrics bar
    const bar = document.getElementById('metrics-bar');
    let html = '';
    for (const [k, v] of Object.entries(data.metrics || {})) {
        const label = k.replace('total_', '').replace('trade.', '');
        html += `<div class="metric"><span class="label">${label}</span><span class="value">${Math.round(v)}</span></div>`;
    }
    bar.innerHTML = html;

    // Update agents
    agentLocations = {};
    const tbody = document.getElementById('agents-body');
    let ahtml = '';
    for (const a of (data.agents || [])) {
        agentLocations[a.id] = a.location;
        const cargo = Object.entries(a.inventory || {})
            .filter(([k,v]) => k !== 'fuel')
            .map(([k,v]) => `${k}:${Math.round(v)}`).join(' ');
        const fuel = a.inventory?.fuel ? `⛽${Math.round(a.inventory.fuel)}` : '';
        ahtml += `<tr class="agent-row">
            <td class="id">${a.id}</td>
            <td class="loc">${a.location.replace('sys_','S')}</td>
            <td class="credits">${Math.round(a.credits)}</td>
            <td>${fuel} ${cargo}</td>
        </tr>`;
    }
    tbody.innerHTML = ahtml;

    // Update charts
    if (data.series) {
        const credits = data.series.total_credits || [];
        const fuel = data.series.total_fuel || [];
        ecoChart.data.labels = credits.map(p => p[0]);
        ecoChart.data.datasets[0].data = credits.map(p => p[1]);
        ecoChart.data.datasets[1].data = fuel.map(p => p[1]);
        ecoChart.update();
    }

    // Events log
    const log = document.getElementById('events-log');
    let ehtml = '';
    for (const ev of (data.events || []).reverse()) {
        let cls = 'event';
        if (ev.key.includes('trade')) cls += ' trade';
        else if (ev.key.includes('move') || ev.key.includes('fuel')) cls += ' move';
        else if (ev.key.includes('extract')) cls += ' extract';
        ehtml += `<div class="${cls}"><span class="t">${ev.tick}</span><span class="k">${ev.key}</span><span class="v">${ev.value}</span></div>`;
    }
    log.innerHTML = ehtml;

    // Speed display
    document.getElementById('speed-display').textContent =
        data.speed === 0 ? 'MAX' : data.speed + ' t/s';
    document.getElementById('btn-pause').textContent = data.paused ? 'RESUME' : 'PAUSE';
    document.getElementById('btn-pause').classList.toggle('active', data.paused);

    drawGalaxy();
};

function cmd(body) { fetch('/command', { method: 'POST', body: JSON.stringify(body) }); }
function togglePause() { paused = !paused; cmd({ paused }); }
function stepOne() { cmd({ step: true }); }
function setSpeed(s) { currentSpeed = s; cmd({ speed: s }); }

window.addEventListener('resize', drawGalaxy);
</script>
</body>
</html>"""
