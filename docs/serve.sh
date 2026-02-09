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

    # Rebuild playground examples from examples/*.rk
    node playground/build-examples.js

    # Copy playground to /app/ if it exists
    if [ -d "playground/pkg" ]; then
        mkdir -p build/app
        cp -r playground/*.html playground/*.js playground/*.css playground/*.png build/app/ 2>/dev/null || true
        cp -r playground/pkg build/app/
    fi

    # Build blog with Jekyll
    if command -v bundle &>/dev/null; then
        cd blog
        bundle exec jekyll build --destination ../build/blog --quiet 2>/dev/null
        cd ..
    else
        echo "Warning: bundle not found, skipping blog build (install ruby + bundle install in docs/blog/)"
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

# Cleanup on exit
trap "kill $SERVER_PID 2>/dev/null" EXIT

# Watch for changes and rebuild
while true; do
    inotifywait -qr -e modify,create,delete book/src landing/ blog/_posts blog/_config.yml blog/assets ../examples/ 2>/dev/null && {
        echo ""
        build_site
    }
done
