#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
set -e

cd "$(dirname "$0")"

build_site() {
    echo "Building website..."

    # The palette lives in shared/tokens.css and is served from the site root.
    # The book's theme @imports it from there, so it has to exist before mdbook
    # runs for the book to come out in the right colours.
    mkdir -p build
    cp shared/tokens.css build/tokens.css

    cd book
    mdbook build -d ../build/book
    cd ..

    cp landing/index.html build/index.html
    cp landing/landing.css build/landing.css

    # Rebuild playground examples from examples/*.rk
    node playground/build-examples.js

    # Copy playground to /app/ if it exists
    if [ -d "playground/pkg" ]; then
        mkdir -p build/app
        cp -r playground/*.html playground/*.js playground/*.css playground/*.png build/app/ 2>/dev/null || true
        cp -r playground/pkg build/app/
    fi

    # The blog became the book's Notes section; keep the old URL working.
    mkdir -p build/blog
    cp landing/blog-redirect.html build/blog/index.html

    echo "Build complete at $(date +%H:%M:%S)"
}

# Initial build
build_site

echo ""
echo "Server running at http://localhost:8080"
echo "Watching for changes in book/, landing/ and shared/..."
echo "Press Ctrl+C to stop"
echo ""

# Start server in background
python3 -m http.server 8080 --directory build &
SERVER_PID=$!

# Cleanup on exit
trap "kill $SERVER_PID 2>/dev/null" EXIT

# Watch for changes and rebuild
while true; do
    inotifywait -qr -e modify,create,delete book/src book/theme landing/ shared/ ../examples/ 2>/dev/null && {
        echo ""
        build_site
    }
done
