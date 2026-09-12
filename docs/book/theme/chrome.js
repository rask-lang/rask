// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Two things mdBook's template gets wrong for this site, fixed after book.js
// has done its own setup.
(function() {
    'use strict';

    // mdBook offers five themes; the site has two palettes, so three of those
    // buttons change nothing you can see. Keep Auto, Light and one dark, and
    // call it Dark. book.js has already read the full list by the time this
    // runs, so a reader with rust/coal/ayu saved keeps the theme they picked.
    var KEEP = {
        'mdbook-theme-default_theme': 'Auto',
        'mdbook-theme-light': 'Light',
        'mdbook-theme-navy': 'Dark'
    };

    document.querySelectorAll('#mdbook-theme-list button.theme').forEach(function(button) {
        var label = KEEP[button.id];
        if (label) {
            button.textContent = label;
        } else {
            var row = button.closest('li');
            if (row) { row.remove(); }
        }
    });

    // The book is one page of a site, and had no way back to the rest of it.
    var bar = document.querySelector('.left-buttons');
    if (bar) {
        var home = document.createElement('a');
        home.className = 'rask-home';
        home.href = '/';
        home.textContent = 'Rask';
        home.title = 'Rask home';
        bar.prepend(home);
    }
})();
