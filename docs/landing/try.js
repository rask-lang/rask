// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// The "Try Rask" box on the front page.
//
// The interpreter is a 16 MB wasm module. Loading that to render a landing page
// would be indefensible, so nothing loads until the first Run — the box is a
// textarea and a button until someone actually wants to run something, and then
// it says what it's doing while the module arrives.
//
// No editor library either. CodeMirror is the right call in the full playground,
// where you write real programs; here it would be a second download for a
// fifteen-line snippet, and a textarea takes the same typing.

// What the box starts with.
//
// Picked for what you can't get elsewhere rather than for being short: the
// `mutate` marker is written twice, once in the signature and once at every
// call, so a reader can see which calls change their argument without opening
// them. That's the whole design in one screen — the safety is in the source,
// not in something the compiler knows and you don't.
//
// Lines stay under about 48 characters so the box needs no sideways scroll on
// a phone.
const SNIPPET = `struct Cart {
    items: Vec<string>
    total: i64
}

// \`mutate\` is written twice: in the signature,
// and again at the call. So you can see which
// calls change their argument.
func add(mutate cart: Cart, item: string, price: i64) {
    cart.items.push(item)
    cart.total += price
}

func main() {
    mut cart = Cart { items: Vec.new(), total: 0 }

    add(mutate cart, "coffee", 45)
    add(mutate cart, "beans", 120)

    println("{cart.items.len()} items, {cart.total} kr")
}`;

const PLAYGROUND = '/app/';

// A textarea can't colour its own text, so the box is a highlighted <pre> with
// a transparent textarea sitting exactly on top of it: you type into the
// textarea, you read the <pre>. The two have to agree on every metric that
// moves a glyph — font, size, line height, padding, wrapping — or the caret
// drifts away from the letters, so the CSS sets those on both together.
//
// Words come from the shared vocabulary (a plain script in the page head), the
// same one the book and the playground read.
const VOCAB = window.RASK_VOCAB;
const KEYWORDS = new Set(VOCAB.keywords.concat(VOCAB.literals));
const TYPES = new Set(VOCAB.types);
const BUILTINS = new Set(VOCAB.builtins);

const escapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;' };
const escape = s => s.replace(/[&<>]/g, c => escapes[c]);

const span = (cls, text) => `<span class="${cls}">${escape(text)}</span>`;

/// Colour Rask source, in the same classes the page's static code windows use.
///
/// Deliberately not a parser. It reads comments, strings, numbers and words,
/// which is everything a fifteen-line snippet shows, and leaves the rest
/// plain. Anything subtler belongs in the playground, where CodeMirror is.
function highlight(src) {
    // One pass, alternatives in priority order: a comment swallows the rest of
    // its line, a string swallows to its closing quote, and only then do we
    // look at words. Otherwise `func` inside a string would come out coloured.
    const pattern = /(\/\/[^\n]*)|("(?:[^"\\\n]|\\.)*")|(\b\d[\d_]*(?:\.\d+)?\b)|(@[A-Za-z_]\w*)|([A-Za-z_]\w*)/g;
    let out = '';
    let last = 0;
    let m;

    while ((m = pattern.exec(src)) !== null) {
        out += escape(src.slice(last, m.index));
        last = pattern.lastIndex;

        const [text, comment, string, number, annotation, word] = m;
        if (comment) { out += span('cmt', text); }
        else if (string) { out += span('str', text); }
        else if (number) { out += span('num', text); }
        else if (annotation) { out += span('ty', text); }
        else if (KEYWORDS.has(word)) { out += span('kw', text); }
        else if (TYPES.has(word)) { out += span('ty', text); }
        else if (BUILTINS.has(word)) { out += span('fn', text); }
        else if (/^[A-Z]/.test(word)) { out += span('ty', text); }
        else if (src[pattern.lastIndex] === '(') { out += span('fn', text); }
        else { out += escape(text); }
    }

    // A trailing newline would leave the <pre>'s last line unpainted as you
    // type it, so the layers end at the same height.
    return out + escape(src.slice(last)) + '\n';
}

let wasm = null;
let loading = null;

/// Fetch and start the interpreter, once.
function interpreter() {
    if (!loading) {
        loading = import('/app/pkg/rask_wasm.js')
            .then(async module => {
                await module.default();
                wasm = new module.Playground();
                return wasm;
            })
            .catch(err => {
                // Let the next press try again rather than wedging the box.
                loading = null;
                throw err;
            });
    }
    return loading;
}

function grow(area) {
    area.style.height = 'auto';
    area.style.height = `${area.scrollHeight}px`;
}

export function mountTry(root) {
    if (!root) { return; }

    root.innerHTML = `
        <div class="try-editor">
            <pre class="try-paint" aria-hidden="true"><code></code></pre>
            <textarea spellcheck="false" autocapitalize="off" autocomplete="off"
                      autocorrect="off" aria-label="Rask code"></textarea>
        </div>
        <div class="try-bar">
            <button type="button" class="btn solid try-run">Run</button>
            <a class="try-open" href="${PLAYGROUND}">Open in the playground &rarr;</a>
        </div>
        <pre class="try-output" aria-live="polite"></pre>`;

    const area = root.querySelector('textarea');
    const paint = root.querySelector('.try-paint > code');
    const run = root.querySelector('.try-run');
    const open = root.querySelector('.try-open');
    const out = root.querySelector('.try-output');

    function repaint() {
        paint.innerHTML = highlight(area.value);
    }

    area.value = SNIPPET;
    repaint();
    grow(area);
    area.addEventListener('input', () => {
        repaint();
        grow(area);
        // Carry whatever is in the box through to the full playground.
        open.href = `${PLAYGROUND}?code=${btoa(encodeURIComponent(area.value))}`;
    });
    // The textarea scrolls on its own when a line runs long; the paint layer
    // has to follow or the two drift apart.
    area.addEventListener('scroll', () => {
        paint.parentElement.scrollTop = area.scrollTop;
        paint.parentElement.scrollLeft = area.scrollLeft;
    });

    function say(text, kind) {
        out.textContent = text;
        out.className = `try-output${kind ? ` ${kind}` : ''}`;
    }

    async function press() {
        run.disabled = true;
        say(wasm ? 'Running…' : 'Starting the interpreter…');

        try {
            const rask = await interpreter();
            const result = rask.run(area.value);
            say(result || '(no output)', 'ok');
        } catch (error) {
            // `Playground::run` answers `Result<String, String>`, so a compiler
            // or runtime diagnostic arrives as a string, already escaped by the
            // Rust side with spans for its colours. Anything else means the
            // module itself fell over, or never arrived.
            if (typeof error === 'string') {
                // The spans inside do the colouring, so the block keeps the
                // normal code foreground. `bad` would paint the source line
                // and the text after `fix:` red along with everything else.
                out.innerHTML = error;
                out.className = 'try-output diag';
            } else {
                say(`Couldn't start the interpreter: ${error.message || error}\n\n` +
                    'The full playground may have better luck.', 'bad');
            }
        } finally {
            run.disabled = false;
        }
    }

    run.addEventListener('click', press);
    area.addEventListener('keydown', event => {
        if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
            event.preventDefault();
            press();
        }
    });
}

mountTry(document.getElementById('try-rask'));
