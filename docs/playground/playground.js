// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Rask Playground - Browser-based code execution

import { EditorView } from 'https://esm.sh/@codemirror/view@6';
import { EditorState } from 'https://esm.sh/@codemirror/state@6';
import { basicSetup } from 'https://esm.sh/@codemirror/basic-setup@0.20';
import { javascript } from 'https://esm.sh/@codemirror/lang-javascript@6';
import { oneDark } from 'https://esm.sh/@codemirror/theme-one-dark@6';

// Global state
let playground = null;
let editor = null;

// Default example code
const DEFAULT_CODE = `func main() {
    println("Hello from Rask!")

    const items = Vec.of(1, 2, 3, 4, 5)
    let sum = 0

    for item in items {
        sum = sum + item
    }

    println("Sum:", sum)
}`;

// Example programs
const EXAMPLES = {
    hello_world: `func main() {
    println("Hello, World!")
    println("Welcome to Rask!")
}`,
    collections: `func main() {
    // Vector
    const numbers = Vec.of(1, 2, 3, 4, 5)
    println("Numbers:", numbers)

    // Struct
    const person = Person {
        name: "Alice",
        age: 30
    }
    println("Person:", person.name, "age:", person.age)

    // Pattern matching
    const result = match person.age {
        0...17 => "Minor",
        18...64 => "Adult",
        _ => "Senior"
    }
    println("Category:", result)
}

struct Person {
    name: string,
    age: i32
}`,
    patterns: `func main() {
    const values = Vec.of(1, 2, 3, 4, 5)

    for value in values {
        const msg = match value {
            1 => "one",
            2 => "two",
            3 => "three",
            _ => "many"
        }
        println(value, "=", msg)
    }

    const opt = Option.Some(42)
    if opt is Some(x) {
        println("Got value:", x)
    }
}`,
    math_demo: `func main() {
    const pi = 3.14159
    const radius = 5.0

    const area = pi * radius * radius
    println("Circle area:", area)

    const nums = Vec.of(10, 20, 30, 40, 50)
    let total = 0

    for n in nums {
        total = total + n
    }

    const avg = total / nums.len()
    println("Average:", avg)
}`
};

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

        // Load code from URL or use default
        loadFromURL();

        // Set up event listeners
        document.getElementById('run-btn').addEventListener('click', runCode);
        document.getElementById('reset-btn').addEventListener('click', resetEditor);
        document.getElementById('share-btn').addEventListener('click', shareCode);
        document.getElementById('clear-output-btn').addEventListener('click', clearOutput);
        document.getElementById('examples').addEventListener('change', loadExample);

        // Keyboard shortcuts
        document.addEventListener('keydown', (e) => {
            if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
                e.preventDefault();
                runCode();
            }
        });

        showLoading(false);
        showToast('Playground ready! Press Ctrl+Enter to run.', 'success');

    } catch (error) {
        showLoading(false);
        showError('Failed to initialize playground: ' + error.message);
        console.error('Init error:', error);
    }
}

// Initialize CodeMirror editor
function initEditor() {
    const startState = EditorState.create({
        doc: DEFAULT_CODE,
        extensions: [
            basicSetup,
            javascript(), // Use JavaScript highlighting as placeholder
            oneDark,
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
        output.textContent = error.toString();
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
