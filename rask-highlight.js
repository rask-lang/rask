// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// Colour Rask source for the site's static pages: the Try box on the front
// page and the showcase pages. Reads the shared vocabulary, so the page has
// to load rask-vocabulary.js first.
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
export function highlight(src) {
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
