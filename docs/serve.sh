#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
set -e

cd "$(dirname "$0")"

build_site() {
    echo "Building website..."

    # Build mdBook to build/book
    cd book
    mdbook build -d ../build/book
    cd ..

    # Copy landing page to root
    cp landing/index.html build/index.html
    cp landing/landing.css build/landing.css

    # Copy playground to /app/ if it exists
    if [ -d "playground/pkg" ]; then
        mkdir -p build/app
        cp -r playground/*.html playground/*.js playground/*.css playground/*.png build/app/ 2>/dev/null || true
        cp -r playground/pkg build/app/
    fi

    echo "Build complete at $(date +%H:%M:%S)"
}

# Initial build
build_site

echo ""
echo "Server running at http://localhost:8080"
echo "Watching for changes in book/ and landing/..."
echo "Press Ctrl+C to stop"
echo ""

# Start server in background
python3 -m http.server 8080 --directory build &
SERVER_PID=$!

# Watch for changes and rebuild
while true; do
    inotifywait -qr -e modify,create,delete book/src landing/ 2>/dev/null && {
        echo ""
        build_site
    }
done

# Cleanup on exit
trap "kill $SERVER_PID 2>/dev/null" EXIT
