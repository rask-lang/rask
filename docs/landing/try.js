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
            <textarea spellcheck="false" autocapitalize="off" autocomplete="off"
                      autocorrect="off" aria-label="Rask code"></textarea>
        </div>
        <div class="try-bar">
            <button type="button" class="btn solid try-run">Run</button>
            <a class="try-open" href="${PLAYGROUND}">Open in the playground &rarr;</a>
        </div>
        <pre class="try-output" aria-live="polite"></pre>`;

    const area = root.querySelector('textarea');
    const run = root.querySelector('.try-run');
    const open = root.querySelector('.try-open');
    const out = root.querySelector('.try-output');

    area.value = SNIPPET;
    grow(area);
    area.addEventListener('input', () => {
        grow(area);
        // Carry whatever is in the box through to the full playground.
        open.href = `${PLAYGROUND}?code=${btoa(encodeURIComponent(area.value))}`;
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
                out.innerHTML = error;
                out.className = 'try-output bad';
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
