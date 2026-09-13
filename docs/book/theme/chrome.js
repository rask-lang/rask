// SPDX-License-Identifier: (MIT OR Apache-2.0)

// What mdBook's template gets wrong for this site, fixed after book.js has
// done its own setup.
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

    // A paint brush is for applying one colour. The button picks between
    // palettes, which is what a palette is.
    var themeButton = document.querySelector('#mdbook-theme-toggle .fa-svg');
    if (themeButton) {
        themeButton.innerHTML =
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor" ' +
            'aria-hidden="true"><path d="M12 2a10 10 0 0 0 0 20c.93 0 1.68-.75 1.68-1.68 0-.44-.17-.83-.44-1.13' +
            '-.27-.29-.43-.68-.43-1.11 0-.93.75-1.68 1.68-1.68H16.4A5.6 5.6 0 0 0 22 10.8C22 5.94 17.52 2 12 2z' +
            'm-5.6 10a1.6 1.6 0 1 1 0-3.2 1.6 1.6 0 0 1 0 3.2zm3.2-4.3a1.6 1.6 0 1 1 0-3.2 1.6 1.6 0 0 1 0 3.2z' +
            'm4.8 0a1.6 1.6 0 1 1 0-3.2 1.6 1.6 0 0 1 0 3.2zm3.2 4.3a1.6 1.6 0 1 1 0-3.2 1.6 1.6 0 0 1 0 3.2z"/></svg>';
    }

    // Tapping outside the open sidebar closes it.
    //
    // The button is a `<label>` for a hidden checkbox, so this is one too: the
    // browser toggles the checkbox, and book.js's own `change` listener keeps
    // its state in step. No second source of truth, and no JS of ours running
    // on the tap.
    //
    // It earns its place on a phone. mdBook opens the sidebar by translating
    // the whole page 308px right, which on a 390px screen leaves 82px of page
    // and puts the toggle 17px from the edge — so the way out is a sliver at
    // the screen edge, if you find it. The CSS overlays the sidebar instead at
    // that width; this is what you tap to dismiss it.
    if (!document.querySelector('.rask-scrim')) {
        var scrim = document.createElement('label');
        scrim.className = 'rask-scrim';
        scrim.setAttribute('for', 'mdbook-sidebar-toggle-anchor');
        scrim.setAttribute('aria-hidden', 'true');
        document.body.appendChild(scrim);
    }
})();
