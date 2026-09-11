// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Rask Playground - Browser-based code execution
import { EditorView, basicSetup } from 'https://esm.sh/codemirror';
import { StreamLanguage, HighlightStyle, syntaxHighlighting } from 'https://esm.sh/@codemirror/language@6';
import { tags } from 'https://esm.sh/@lezer/highlight@1';
import { EditorState } from 'https://esm.sh/@codemirror/state@6';
import { keymap } from 'https://esm.sh/@codemirror/view@6';
import { indentWithTab } from 'https://esm.sh/@codemirror/commands@6';
import { linter } from 'https://esm.sh/@codemirror/lint@6';
import { EXAMPLES, EXAMPLE_METADATA, DEFAULT_CODE } from './examples.js';

// Rask language definition for CodeMirror
const raskLanguage = StreamLanguage.define({
    name: "rask",
    startState: () => ({ inComment: false }),
    token: (stream, state) => {
        // Comments
        if (stream.match("//")) {
            stream.skipToEnd();
            return "comment";
        }

        // Strings
        if (stream.match(/^"(?:[^"\\]|\\.)*"/)) {
            return "string";
        }

        // Numbers
        if (stream.match(/^[0-9]+\.?[0-9]*/)) {
            return "number";
        }

        // Keywords
        if (stream.match(/^(func|let|mut|const|if|else|match|loop|while|for|in|is|as|return|struct|enum|trait|extend|union|public|private|try|catch|ensure|with|using|comptime|take|read|mutate|own|where|unsafe|break|continue|spawn|import|export|type|test|assert)\b/)) {
            return "keyword";
        }

        // Types
        if (stream.match(/^(i8|i16|i32|i64|u8|u16|u32|u64|usize|isize|f32|f64|bool|string|char|void|none|Vec|Map|Set|Pool|Handle|Rack|Link|Shared|Heap|Atomic|StringView)\b/)) {
            return "type";
        }

        // Builtins
        if (stream.match(/^(println|print|format|assert|panic)\b/)) {
            return "builtin";
        }

        // Operators
        if (stream.match(/^[+\-*\/%=<>!&|^?]/)) {
            return "operator";
        }

        stream.next();
        return null;
    }
});

// Editor theme. Reads the site's colour tokens rather than restating them, so
// the editor tracks the palette (and the reader's light/dark preference) with
// the rest of the site.
const token = name => getComputedStyle(document.documentElement)
    .getPropertyValue(name).trim();

function raskEditorTheme() {
    return EditorView.theme({
        "&": {
            height: "100%",
            backgroundColor: token('--code-bg'),
            color: token('--code-fg'),
            fontSize: "0.85rem",
        },
        ".cm-scroller": { overflow: "auto", fontFamily: token('--font-mono'), lineHeight: "1.7" },
        ".cm-content": { caretColor: token('--accent') },
        ".cm-cursor, .cm-dropCursor": { borderLeftColor: token('--accent') },
        ".cm-gutters": {
            backgroundColor: token('--code-chrome'),
            color: token('--c-cmt'),
            border: "none",
            borderRight: `1px solid ${token('--code-rule')}`,
        },
        ".cm-activeLine": { backgroundColor: "rgba(255, 255, 255, 0.03)" },
        ".cm-activeLineGutter": { backgroundColor: "transparent", color: token('--code-fg') },
        "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
            backgroundColor: "rgba(89, 190, 139, 0.22)",
        },
        ".cm-selectionMatch": { backgroundColor: "rgba(89, 190, 139, 0.15)" },
        ".cm-tooltip": {
            backgroundColor: token('--code-chrome'),
            border: `1px solid ${token('--code-rule')}`,
            color: token('--code-fg'),
        },
        ".cm-lintRange-error": { backgroundImage: "none", borderBottom: "2px solid #e8877c" },
        ".cm-lintRange-warning": { backgroundImage: "none", borderBottom: `2px solid ${token('--c-kw')}` },
    }, { dark: true });
}

function raskHighlightStyle() {
    return HighlightStyle.define([
        { tag: tags.comment, color: token('--c-cmt'), fontStyle: "italic" },
        { tag: tags.keyword, color: token('--c-kw') },
        { tag: tags.typeName, color: token('--c-ty') },
        { tag: tags.string, color: token('--c-str') },
        { tag: tags.number, color: token('--c-str') },
        { tag: tags.variableName, color: token('--code-fg') },
        { tag: tags.operator, color: token('--c-cmt') },
        // The Rask tokenizer labels println/print/format as builtins.
        { tag: tags.standard(tags.variableName), color: token('--c-fn') },
    ]);
}

// Rask syntax error linter
function raskLinter(view) {
    if (!playground) return [];

    try {
        const code = view.state.doc.toString();
        const diagnosticsJson = playground.check(code);
        const report = JSON.parse(diagnosticsJson);

        return report.diagnostics.map(diag => {
            // Find primary label for position
            const primaryLabel = diag.labels.find(l => l.role === "primary") || diag.labels[0];

            return {
                from: primaryLabel.start.byte_offset,
                to: primaryLabel.end.byte_offset,
                severity: diag.severity.toLowerCase(), // "error" or "warning"
                message: diag.message
            };
        });
    } catch (error) {
        console.error('Linter error:', error);
        return [];
    }
}

// Global state
let playground = null;
let editor = null;

// Initialize playground
async function init() {
    try {
        showLoading(true);

        // Load WASM module
        const wasm = await import('./pkg/rask_wasm.js');
        await wasm.default();
        playground = new wasm.Playground();

        // Get version
        const version = wasm.Playground.version();
        document.getElementById('version').textContent = version;

        // Initialize editor
        initEditor();

        // Populate examples dropdown
        populateExamples();

        // Load code from URL or use default
        loadFromURL();

        // Set up event listeners
        document.getElementById('run-btn').addEventListener('click', runCode);
        document.getElementById('reset-btn').addEventListener('click', resetEditor);
        document.getElementById('share-btn').addEventListener('click', shareCode);
        document.getElementById('clear-output-btn').addEventListener('click', clearOutput);
        document.getElementById('examples').addEventListener('change', loadExample);

        showLoading(false);
        window.__raskPlaygroundReady = true;
        showToast('Ready. Ctrl+Enter runs.', 'success');

    } catch (error) {
        showLoading(false);
        showError('Failed to initialize playground: ' + error.message);
        console.error('Init error:', error);
    }
}

// Populate examples dropdown from metadata.
//
// `needsLocal` is set by build-examples.js for anything that reads files, the
// clock, sockets or threads — none of which exist in a browser. Those go in
// their own group rather than being mixed in with the ones that run, so the
// reader isn't finding out by clicking.
function populateExamples() {
    const dropdown = document.getElementById('examples');
    dropdown.innerHTML = '<option value="">Load an example\u2026</option>';

    const addGroup = (label, items) => {
        if (items.length === 0) return;
        const group = document.createElement('optgroup');
        group.label = label;
        items.forEach(ex => {
            const option = document.createElement('option');
            option.value = ex.key;
            option.textContent = ex.title;
            group.appendChild(option);
        });
        dropdown.appendChild(group);
    };

    const runsHere = EXAMPLE_METADATA.filter(ex => !ex.needsLocal);
    addGroup('Learn the language', runsHere.filter(ex => ex.key.match(/^\d+_/)));
    addGroup('Whole programs', runsHere.filter(ex => !ex.key.match(/^\d+_/)));
    addGroup('Need a local install', EXAMPLE_METADATA.filter(ex => ex.needsLocal));
}

// Initialize CodeMirror editor
function initEditor() {
    // Custom keymap for Ctrl+Enter to run code
    const runKeymap = keymap.of([
        {
            key: "Ctrl-Enter",
            run: () => {
                runCode();
                return true;
            }
        },
        {
            key: "Cmd-Enter",  // For Mac
            run: () => {
                runCode();
                return true;
            }
        }
    ]);

    const startState = EditorState.create({
        doc: DEFAULT_CODE,
        extensions: [
            basicSetup,
            raskLanguage,
            raskEditorTheme(),
            syntaxHighlighting(raskHighlightStyle()),
            linter(raskLinter, { delay: 300 }),
            keymap.of([indentWithTab]),
            runKeymap
        ]
    });

    editor = new EditorView({
        state: startState,
        parent: document.getElementById('editor')
    });
}

// Run code
async function runCode() {
    if (!playground) {
        showError('Playground not initialized');
        return;
    }

    const code = editor.state.doc.toString();
    const output = document.getElementById('output');

    // Clear previous output
    output.textContent = 'Running...';
    output.className = 'output-content running';

    try {
        const result = playground.run(code);
        output.textContent = result || '(no output)';
        output.className = 'output-content success';
    } catch (error) {
        output.className = 'output-content error';

        // `Playground::run` answers `Result<String, String>`, so a compiler or
        // runtime diagnostic arrives as a JS string — already HTML-escaped by
        // the Rust side, with spans for the ANSI colours. Anything else is an
        // object, and means the interpreter itself fell over.
        if (typeof error === 'string') {
            output.innerHTML = error;
            return;
        }

        output.textContent =
            'The interpreter crashed on this program. That is a compiler bug, not your code:\n\n' +
            `  ${error}\n\n` +
            'Please report it at https://github.com/rask-lang/rask/issues — the program above\n' +
            'is the whole repro. Restarting the interpreter; your code is untouched.';
        await reviveInterpreter();
    }
}

// Replace a wasm instance that has stopped being usable.
//
// A Rust panic or trap inside the module skips every destructor, so
// wasm-bindgen's mutable borrow of `Playground` is never handed back: every
// later call answers "recursive use of an object detected" instead of running
// anything. Nothing on the JS side can release that borrow, and the fix used to
// be for the reader to guess they should reload the page.
async function reviveInterpreter() {
    try {
        const wasm = await import('./pkg/rask_wasm.js');
        playground = new wasm.Playground();
    } catch (error) {
        playground = null;
        console.error('Could not restart the interpreter:', error);
        showToast('Could not restart the interpreter — reload the page', 'error');
    }
}

// Reset editor to default code
function resetEditor() {
    editor.dispatch({
        changes: {
            from: 0,
            to: editor.state.doc.length,
            insert: DEFAULT_CODE
        }
    });
    clearOutput();
    showToast('Reset.');
}

// Clear output
function clearOutput() {
    const output = document.getElementById('output');
    output.textContent = 'Press Run, or Ctrl+Enter.';
    output.className = 'output-content';
}

// Load example
function loadExample(e) {
    const example = e.target.value;
    if (!example) return;

    const code = EXAMPLES[example];
    if (code) {
        editor.dispatch({
            changes: {
                from: 0,
                to: editor.state.doc.length,
                insert: code
            }
        });

        const meta = EXAMPLE_METADATA.find(ex => ex.key === example);
        const output = document.getElementById('output');
        if (meta && meta.needsLocal) {
            output.textContent =
                `This one ${meta.needsLocal}, and a browser can do none of that.\n\n` +
                'It is here to read. To run it, install Rask and use the file in examples/.';
            output.className = 'output-content';
        } else {
            clearOutput();
        }
        showToast(`Loaded ${example.replace(/_/g, ' ')}`);
    }

    // Reset dropdown
    e.target.value = '';
}

// Share code via URL
function shareCode() {
    const code = editor.state.doc.toString();
    const encoded = btoa(encodeURIComponent(code));
    const url = `${window.location.origin}${window.location.pathname}?code=${encoded}`;

    navigator.clipboard.writeText(url).then(() => {
        showToast('Link copied.', 'success');
    }).catch(() => {
        // Fallback: show URL in prompt
        prompt('Copy this link:', url);
    });
}

// Load code from URL parameter
function loadFromURL() {
    const params = new URLSearchParams(window.location.search);
    const encoded = params.get('code');

    if (encoded) {
        try {
            const code = decodeURIComponent(atob(encoded));
            editor.dispatch({
                changes: {
                    from: 0,
                    to: editor.state.doc.length,
                    insert: code
                }
            });
        } catch (error) {
            showError('Failed to load code from URL');
            console.error('URL decode error:', error);
        }
    }
}

// UI helpers
function showLoading(show) {
    document.getElementById('loading-overlay').style.display = show ? 'flex' : 'none';
}

function showError(message) {
    const output = document.getElementById('output');
    output.textContent = 'Error: ' + message;
    output.className = 'output-content error';
}

function showToast(message, type = 'info') {
    const toast = document.getElementById('toast');
    toast.textContent = message;
    toast.className = `toast show ${type}`;

    setTimeout(() => {
        toast.className = 'toast';
    }, 3000);
}

// Start playground when page loads
window.addEventListener('load', init);
