// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask syntax highlighting for mdBook
(function() {
    'use strict';

    // Define Rask language for highlight.js
    function defineRask(hljs) {
        return {
            name: 'Rask',
            aliases: ['rk'],
            keywords: {
                keyword:
                    'func struct enum trait extend union match if else for while loop return ' +
                    'let mut const try catch ensure comptime with spawn using import export ' +
                    'public private take read mutate own where unsafe extern break continue ' +
                    'is in as select type lazy test benchmark assert check package dep',
                literal: 'true false none null',
                built_in:
                    'string i8 i16 i32 i64 u8 u16 u32 u64 usize isize f32 f64 bool char void ' +
                    'Vec Map Set Pool Handle Rack Link Shared Heap Atomic StringView ' +
                    'println print format panic todo unreachable'
            },
            contains: [
                hljs.COMMENT('//', '$'),
                hljs.COMMENT('/\\*', '\\*/', {contains: ['self']}),
                {
                    className: 'string',
                    variants: [
                        {begin: /"/, end: /"/, contains: [hljs.BACKSLASH_ESCAPE]},
                        {begin: /'/, end: /'/, contains: [hljs.BACKSLASH_ESCAPE]}
                    ]
                },
                {
                    className: 'number',
                    variants: [
                        {begin: '\\b0b[01_]+'},
                        {begin: '\\b0o[0-7_]+'},
                        {begin: '\\b0x[0-9a-fA-F_]+'},
                        {begin: '\\b\\d+(_\\d+)*(\\.[0-9]+)?([eE][+-]?[0-9]+)?'}
                    ],
                    relevance: 0
                },
                {
                    className: 'title.function',
                    begin: /\b[a-z_][a-z0-9_]*(?=\s*\()/,
                    relevance: 0
                },
                {
                    className: 'title.class',
                    begin: /\b[A-Z][a-zA-Z0-9_]*/,
                    relevance: 0
                },
                {
                    className: 'meta',
                    begin: /@[a-z_][a-z0-9_]*/
                }
            ]
        };
    }

    function highlightRaskBlocks(hljsInstance) {
        document.querySelectorAll('pre code.language-rask, pre code.rask').forEach(function(block) {
            // Try different highlight methods for compatibility
            if (hljsInstance.highlightElement) {
                hljsInstance.highlightElement(block);
            } else if (hljsInstance.highlightBlock) {
                hljsInstance.highlightBlock(block);
            } else if (hljsInstance.highlightAuto) {
                const result = hljsInstance.highlightAuto(block.textContent, ['rask']);
                block.innerHTML = result.value;
                block.className = 'hljs ' + result.language;
            }
        });
    }

    function init() {
        const hljsInstance = typeof hljs !== 'undefined' ? hljs : (typeof window.hljs !== 'undefined' ? window.hljs : null);

        if (hljsInstance && hljsInstance.registerLanguage) {
            console.log('Registering Rask language');
            hljsInstance.registerLanguage('rask', defineRask);

            // Wait a bit for mdBook to finish its own highlighting
            setTimeout(function() {
                highlightRaskBlocks(hljsInstance);
            }, 100);

            return true;
        }

        return false;
    }

    // Try on DOMContentLoaded
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
