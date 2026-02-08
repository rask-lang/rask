#!/bin/bash
# SPDX-License-Identifier: (MIT OR Apache-2.0)
set -e

cd "$(dirname "$0")"

echo "Building website..."

# Build mdBook to build/book
cd book
mdbook build -d ../build/book
cd ..

# Copy landing page to root
echo "Adding landing page..."
cp landing/index.html build/index.html
cp landing/landing.css build/landing.css

# Copy playground to /app/ if it exists
if [ -d "playground/pkg" ]; then
    echo "Adding playground..."
    mkdir -p build/app
    cp -r playground/*.html playground/*.js playground/*.css playground/*.png build/app/ 2>/dev/null || true
    cp -r playground/pkg build/app/
fi

echo ""
echo "Website built successfully!"
echo "Starting server at http://localhost:8080"
echo ""

# Start server
python3 -m http.server 8080 --directory build
