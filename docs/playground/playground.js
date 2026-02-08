// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Rask Playground - Browser-based code execution
import { EditorView, basicSetup } from 'https://esm.sh/codemirror';
import { javascript } from 'https://esm.sh/@codemirror/lang-javascript';
import { oneDark } from 'https://esm.sh/@codemirror/theme-one-dark';
import { EditorState } from 'https://esm.sh/@codemirror/state@6';
import { keymap } from 'https://esm.sh/@codemirror/view@6';
import { indentWithTab } from 'https://esm.sh/@codemirror/commands@6';
import { EXAMPLES, EXAMPLE_METADATA, DEFAULT_CODE } from './examples.js';

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
        showToast('Playground ready! Press Ctrl+Enter to run.', 'success');

    } catch (error) {
        showLoading(false);
        showError('Failed to initialize playground: ' + error.message);
        console.error('Init error:', error);
    }
}

// Populate examples dropdown from metadata
function populateExamples() {
    const dropdown = document.getElementById('examples');

    // Clear existing options except the first one
    dropdown.innerHTML = '<option value="">Examples...</option>';

    // Add learning examples first (01_* through 07_*)
    const learningExamples = EXAMPLE_METADATA.filter(ex => ex.key.match(/^\d+_/));
    if (learningExamples.length > 0) {
        const group = document.createElement('optgroup');
        group.label = 'Learn Rask';
        learningExamples.forEach(ex => {
            const option = document.createElement('option');
            option.value = ex.key;
            option.textContent = ex.title;
            group.appendChild(option);
        });
        dropdown.appendChild(group);
    }

    // Add other examples
    const otherExamples = EXAMPLE_METADATA.filter(ex => !ex.key.match(/^\d+_/));
    if (otherExamples.length > 0) {
        const group = document.createElement('optgroup');
        group.label = 'More Examples';
        otherExamples.forEach(ex => {
            const option = document.createElement('option');
            option.value = ex.key;
            option.textContent = ex.title;
            group.appendChild(option);
        });
        dropdown.appendChild(group);
    }
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
            javascript(), // Use JavaScript highlighting as placeholder
            oneDark,
            keymap.of([indentWithTab]),
            runKeymap,
            EditorView.theme({
                "&": { height: "100%" },
                ".cm-scroller": { overflow: "auto" }
            })
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
        // Error messages may contain HTML from ANSI-to-HTML conversion
        output.innerHTML = error.toString();
        output.className = 'output-content error';
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
    showToast('Editor reset');
}

// Clear output
function clearOutput() {
    const output = document.getElementById('output');
    output.textContent = 'Click "Run" to execute your code...';
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
        clearOutput();
        showToast(`Loaded example: ${example.replace('_', ' ')}`);
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
        showToast('Link copied to clipboard!', 'success');
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
