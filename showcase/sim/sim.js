// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// The sim showcase. Everything shown comes from files next to this page: the
// two programs, and the traces `rask test --sim --trace` wrote for them. A
// test in the compiler's suite regenerates the traces and fails when they
// change, so the page can't drift from what the compiler does.

import { highlight } from '/rask-highlight.js?v=ca602a9ed9f3';

// The program's names for what the trace numbers. Task ids follow spawn
// order, and the bank is the only object the program locks.
const LANES = [
    { task: 0, name: 'test' },
    { task: 1, name: 'to_bob' },
    { task: 2, name: 'to_carol' },
];
const OBJECTS = { 1: 'bank' };

const TRACES = {
    fail: {
        file: 'bank.trace.json',
        note: 'to_bob and to_carol both read 100 before either one subtracts, so both send 80.',
    },
    pass: {
        file: 'bank.pass.trace.json',
        note: 'Same program, different seed. to_carol finishes first, so to_bob reads 20 and stops.',
    },
    fixed: {
        file: 'bank_fixed.trace.json',
        note: 'The fixed program on seed 1. The read and the subtraction happen inside one write lock, so the second transfer reads 20.',
    },
};

const escapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' };
const escape = s => String(s).replace(/[&<>"]/g, c => escapes[c]);
const laneName = id => (LANES.find(l => l.task === id) || { name: `task ${id}` }).name;

async function text(file) {
    const r = await fetch(file);
    if (!r.ok) throw new Error(`${file}: ${r.status}`);
    return r.text();
}

// ─── Source windows ─────────────────────────────────────────

// The file minus its licence header and leading comment, which describe the
// file rather than the program.
function body(src) {
    const lines = src.split('\n');
    let i = 0;
    while (i < lines.length && (lines[i].startsWith('//') || lines[i].trim() === '')) i++;
    return lines.slice(i).join('\n').trimEnd();
}

async function showSource(id, file, only) {
    let src = body(await text(file));
    // Just one function: from its `func` line to the `}` that closes it.
    if (only) {
        const start = src.indexOf(`func ${only}(`);
        const end = src.indexOf('\n}', start);
        src = src.slice(start, end + 2);
    }
    document.querySelector(`#${id} code`).innerHTML = highlight(src);
}

// ─── The sim run, from the trace ────────────────────────────

// `HH:MM:SS.uuuuuu`, as the runner prints it.
function virtualTime(ns) {
    const us = Math.floor(ns / 1000);
    const pad = (n, w) => String(n).padStart(w, '0');
    return `${pad(Math.floor(us / 3.6e9), 2)}:${pad(Math.floor(us / 6e7) % 60, 2)}:` +
        `${pad(Math.floor(us / 1e6) % 60, 2)}.${pad(us % 1e6, 6)}`;
}

function showSimOutput(trace) {
    const printed = trace.events.filter(e => e.op === 'print').map(e => `  │ ${escape(e.text)}`);
    const seed = /--seed (\d+)/.exec(trace.replay)[1];
    document.getElementById('sim-output').innerHTML = [
        `<span class="prompt">$</span> rask test --sim --seed ${seed} bank.rk`,
        '',
        `sim: seed ${seed}`,
        '',
        `<span class="bad">FAIL:</span> ${escape(trace.test)}`,
        `  <span class="bad">${escape(trace.error)}</span>`,
        `  step ${trace.steps}, virtual time ${virtualTime(trace.time_ns)}`,
        '  output:',
        ...printed,
        `  replay: <span class="replay">${escape(trace.replay)}</span>`,
        '',
        '1 tests, 0 passed, 1 failed',
    ].join('\n');
}

// ─── The viewer ─────────────────────────────────────────────

// A read that a later write by the same task acts on, after another task has
// written in between: the write lands on a balance the reader never saw. Read
// off the trace itself, so the page marks whatever trace it is given.
function staleEvents(events) {
    const stale = new Set();
    events.forEach((e, i) => {
        if (e.op !== 'acquire' || e.what !== 'Shared read') return;
        let clobbered = false;
        for (let j = i + 1; j < events.length; j++) {
            const later = events[j];
            if (later.op !== 'acquire' || later.obj !== e.obj || later.what !== 'Shared write') continue;
            if (later.task !== e.task) {
                clobbered = true;
            } else {
                if (clobbered) {
                    stale.add(i);
                    stale.add(j);
                }
                break;
            }
        }
    });
    return stale;
}

// What a row says. Releases and the trailing bookkeeping are left out: a
// `with` block's release follows its acquire at the same step, and the page
// is about who read what when.
function describe(e) {
    const obj = OBJECTS[e.obj] || `object ${e.obj}`;
    switch (e.op) {
        case 'spawn': return { text: `spawn ${laneName(e.other)}`, cls: 'life' };
        case 'done': return { text: 'done', cls: 'life' };
        case 'join': return { text: `join ${laneName(e.other)}`, cls: 'life' };
        case 'panic': return { text: `panic: ${e.text}`, cls: 'bad' };
        case 'print': return { text: `“${e.text}”`, cls: 'print' };
        case 'acquire':
            if (e.what === 'Shared read') return { text: `read ${obj}`, cls: 'lock read' };
            if (e.what === 'Shared write') return { text: `write ${obj}`, cls: 'lock write' };
            return { text: `${e.what} ${obj}`, cls: 'lock' };
        case 'send': case 'receive': return { text: `${e.op} on channel ${e.obj}`, cls: 'life' };
        default: return null;
    }
}

function viewer(traces) {
    const lanes = document.getElementById('lanes');
    const note = document.getElementById('viewer-note');
    const result = document.getElementById('viewer-result');
    const pos = document.getElementById('viewer-pos');
    const play = document.getElementById('play');
    const tabs = [...document.querySelectorAll('.viewer-tabs [role=tab]')];

    let rows = [];
    let shown = 0;
    let timer = null;
    let current = null;

    function render(key) {
        current = key;
        const trace = traces[key];
        const stale = staleEvents(trace.events);
        rows = [];
        trace.events.forEach((e, i) => {
            const d = describe(e);
            if (d) rows.push({ e, d, stale: stale.has(i) });
        });

        const head = `<div class="lane-row lane-head"><span class="step">step</span>${
            LANES.map(l => `<span class="lane-name">${l.name}</span>`).join('')}</div>`;
        const body = rows.map((r, i) => {
            const cells = LANES.map(l => l.task === r.e.task
                ? `<span class="cell"><span class="chip ${r.d.cls}${r.stale ? ' stale' : ''}">${escape(r.d.text)}</span></span>`
                : '<span class="cell"></span>').join('');
            return `<div class="lane-row" data-row="${i}"><span class="step">${r.e.step}</span>${cells}</div>`;
        }).join('');
        lanes.innerHTML = head + body;

        note.textContent = TRACES[key].note;
        document.getElementById('viewer-legend').hidden = stale.size === 0;
        result.innerHTML = trace.passed
            ? `<span class="good">passed</span> after ${trace.steps} steps`
            : `<span class="bad">${escape(trace.error.replace(/^.*?: /, ''))}</span> at step ${trace.steps}`;
        tabs.forEach(t => t.setAttribute('aria-selected', String(t.dataset.trace === key)));
        reveal(rows.length);
    }

    function reveal(n) {
        shown = Math.max(0, Math.min(rows.length, n));
        lanes.querySelectorAll('.lane-row[data-row]').forEach(el => {
            const i = Number(el.dataset.row);
            el.classList.toggle('hidden', i >= shown);
            el.classList.toggle('current', i === shown - 1 && shown < rows.length);
        });
        result.classList.toggle('hidden', shown < rows.length);
        pos.textContent = shown < rows.length ? `${shown} / ${rows.length}` : '';
    }

    function stop() {
        clearInterval(timer);
        timer = null;
        play.textContent = 'Play';
    }

    function start() {
        if (shown >= rows.length) reveal(0);
        play.textContent = 'Pause';
        timer = setInterval(() => {
            reveal(shown + 1);
            if (shown >= rows.length) stop();
        }, 650);
    }

    play.addEventListener('click', () => (timer ? stop() : start()));
    document.getElementById('step-next').addEventListener('click', () => { stop(); reveal(shown + 1); });
    document.getElementById('step-back').addEventListener('click', () => { stop(); reveal(shown - 1); });
    tabs.forEach(t => t.addEventListener('click', () => { stop(); render(t.dataset.trace); }));

    render('fail');

    // Play once, the first time the viewer comes into view, unless the
    // reader asked for less motion.
    const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (!calm && 'IntersectionObserver' in window) {
        const once = new IntersectionObserver(entries => {
            if (entries.some(e => e.isIntersecting)) {
                once.disconnect();
                if (current === 'fail') { reveal(0); start(); }
            }
        }, { threshold: 0.4 });
        once.observe(lanes);
    }
}

// ─── Start ──────────────────────────────────────────────────

async function main() {
    const traces = {};
    await Promise.all(Object.entries(TRACES).map(async ([key, t]) => {
        traces[key] = JSON.parse(await text(t.file));
    }));
    await Promise.all([
        showSource('bank-source', 'bank.rk'),
        showSource('fixed-source', 'bank_fixed.rk', 'transfer'),
    ]);
    showSimOutput(traces.fail);
    viewer(traces);
}

main().catch(err => {
    document.getElementById('lanes').textContent = `Couldn't load the traces: ${err.message}`;
});
