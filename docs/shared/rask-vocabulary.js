// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// Rask's words, in one place.
//
// Three things on this site colour Rask source: the book (highlight.js), the
// playground (CodeMirror), and the Try box on the front page (its own overlay,
// because a textarea can't colour its own text). Each needs a different shape
// of tokenizer, so those stay separate. What they share is the vocabulary, and
// that is the part that rots: the playground was still painting `Pool` and
// `Handle` as types months after both were replaced by `Rack` and `Link`,
// because the book's list had been updated and its own hadn't.
//
// A plain script, not a module, so it can run from the book's <head> ahead of
// highlight.js. Module scripts are deferred and would arrive too late there.
//
// KEYWORDS is read off the `#[token("...")]` entries in
// compiler/crates/rask-lexer/src/lexer.rs. When a keyword is added there it
// belongs here too.
window.RASK_VOCAB = {
    keywords: (
        'as asm assert benchmark break catch check comptime const continue dep ' +
        'discard else ensure enum exclusive export extend extern feature for func ' +
        'if import in is lazy let loop match mut mutate native or own package ' +
        'private profile public read return scope select select_priority struct ' +
        'take test trait try type union unsafe using where while with'
    ).split(' '),

    literals: 'true false none null'.split(' '),

    types: (
        'string i8 i16 i32 i64 u8 u16 u32 u64 usize isize f32 f64 bool char void ' +
        'Vec Map Set Rack Link Shared Heap Owned Cell Mutex StringView'
    ).split(' '),

    builtins: 'println print format panic todo unreachable spawn'.split(' '),
};
