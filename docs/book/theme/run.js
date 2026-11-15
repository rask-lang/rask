// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// Run button for the book's Rask blocks.
//
// Which blocks get one is not a judgement made here. Every ```rask block in
// the book carries a test-specs marker or is an {{#include}} out of a program
// another gate runs, and the book gate fails the build if one carries neither.
// A `test: run | expected` marker means the block is a whole program that
// compiles and prints that — so those get a Run, and nothing else does. An
// {{#include}} block is a few lines lifted out of a larger file with an anchor;
// it has no `main` and would only ever fail.
//
// mdBook renders the marker straight through as an HTML comment sitting just
// before the <pre>, which is how this finds them.
//
// The tour's pages are made from its step files (learn_pages.py), and their
// blocks carry `tour: test` or `tour: run` instead. A step is one program cut
// into pieces by its prose, so every Run on the page runs all of its blocks
// together: the tests for a step with tests, `main` for one without. The
// reader does the step's exercise right there, so those blocks are editable.
//
// The interpreter is a multi-megabyte wasm module. Nothing loads until someone
// presses Run, and then one module serves every block on the page.

(function () {
    'use strict';

    const RUNNABLE = /^\s*test:\s*run\b/;
    const TOUR = /^\s*tour:\s*(test|run)\b/;

    let wasm = null;
    let loading = null;

    function interpreter() {
        if (!loading) {
            // Both URLs carry the build stamp; the binary needs its own,
            // because the glue resolves it relative to `import.meta.url` and
            // that drops the query.
            loading = import('/app/pkg/rask_wasm.js?v=dev')
                .then(async module => {
                    await module.default({ module_or_path: '/app/pkg/rask_wasm_bg.wasm?v=dev' });
                    wasm = new module.Playground();
                    return wasm;
                })
                .catch(err => {
                    // Let the next press try again rather than wedging the page.
                    loading = null;
                    throw err;
                });
        }
        return loading;
    }

    /// The marker for a block, if it has one.
    ///
    /// Walks back over whitespace only: anything else between the comment and
    /// the block means the comment belongs to something else.
    function marker(pre) {
        let node = pre.previousSibling;
        while (node && node.nodeType === Node.TEXT_NODE && !node.data.trim()) {
            node = node.previousSibling;
        }
        return node && node.nodeType === Node.COMMENT_NODE ? node.data : null;
    }

    /// `source()` is the program a press runs, and `tests` says whether to run
    /// its `test` blocks or its `main`.
    function addRun(pre, source, tests) {
        const out = document.createElement('pre');
        out.className = 'rask-output';
        out.hidden = true;
        pre.after(out);

        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'rask-run';
        button.title = tests ? "Run this page's tests" : 'Run this program';
        button.textContent = tests ? 'Test' : 'Run';

        // mdBook hangs its own controls off `.buttons` in the corner of a
        // block. Sit with them rather than inventing a second place to look.
        const buttons = pre.querySelector('.buttons');
        if (buttons) { buttons.prepend(button); } else { pre.prepend(button); }

        function say(text, kind) {
            out.hidden = false;
            out.textContent = text;
            out.className = `rask-output${kind ? ` ${kind}` : ''}`;
        }

        button.addEventListener('click', async () => {
            button.disabled = true;
            say(wasm ? 'Running…' : 'Starting the interpreter…');
            try {
                const rask = await interpreter();
                const program = source();
                say((tests ? rask.run_tests(program) : rask.run(program)) || '(no output)', 'ok');
            } catch (error) {
                // A compiler or runtime diagnostic arrives as a string, already
                // escaped by the Rust side and carrying spans for its colours.
                // Anything else means the module itself fell over.
                if (typeof error === 'string') {
                    out.hidden = false;
                    out.innerHTML = error;
                    out.className = 'rask-output diag';
                } else {
                    say(`Couldn't start the interpreter: ${error.message || error}`, 'bad');
                }
            } finally {
                button.disabled = false;
            }
        });
    }

    function wire() {
        const tour = [];
        document.querySelectorAll('pre > code.language-rask').forEach(code => {
            const pre = code.parentElement;
            if (pre.dataset.raskRun) { return; }
            const found = marker(pre);
            if (!found) { return; }
            const step = TOUR.exec(found);
            if (step) {
                tour.push({ pre, code, tests: step[1] === 'test' });
            } else if (RUNNABLE.test(found)) {
                pre.dataset.raskRun = '1';
                addRun(pre, () => code.textContent, false);
            }
        });
        const whole = () => tour.map(block => block.code.textContent).join('\n\n');
        tour.forEach(block => {
            block.pre.dataset.raskRun = '1';
            // `plaintext-only` keeps a paste from bringing markup along; where
            // a browser lacks it the property throws and plain editing is left.
            try { block.code.contentEditable = 'plaintext-only'; } catch (e) { block.code.contentEditable = 'true'; }
            block.code.spellcheck = false;
            addRun(block.pre, whole, block.tests);
        });
    }

    // book.js adds its own buttons after DOMContentLoaded, so run after it and
    // once more on the next frame to land inside `.buttons` rather than beside
    // it. `wire` is idempotent.
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', () => requestAnimationFrame(wire));
    } else {
        requestAnimationFrame(wire);
    }
})();
