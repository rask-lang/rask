# Rask Playground

Browser-based interactive playground for trying Rask code.

## Architecture

- **Frontend**: CodeMirror 6 editor with One Dark theme
- **Backend**: Rask interpreter compiled to WASM via wasm-pack
- **Features**: Live execution, example programs, URL sharing

## Development

### Building Examples

Examples are loaded from `../../examples/*.rk` files. When you add or modify examples:

```bash
cd docs/playground
node build-examples.js
```

This generates `examples.js` with all examples organized into:
- **Learn Rask** (01-07): Pedagogical examples teaching concepts
- **More Examples**: Full programs (HTTP server, grep clone, etc.)

### Adding New Examples

1. Create a `.rk` file in `examples/`:
   - Learning examples: Use prefix `01_`, `02_`, etc.
   - Other examples: Use descriptive names

2. Rebuild examples:
   ```bash
   node build-examples.js
   ```

3. Examples automatically populate the dropdown

### Building WASM

```bash
cd compiler/crates/rask-wasm
wasm-pack build --target web
```

Copy output to `docs/playground/pkg/`.

### Local Development

Serve with any static server:
```bash
python3 -m http.server 8000
# or
npx serve .
```

Open http://localhost:8000

## Files

- `index.html` - Main playground UI
- `playground.js` - CodeMirror setup, WASM integration, event handlers
- `playground.css` - Styling
- `examples.js` - **Auto-generated** from `examples/*.rk`
- `build-examples.js` - Build script for examples
- `pkg/` - WASM module and bindings

## Deployment

Deployed to GitHub Pages via `.github/workflows/gh-pages.yml`:
- Builds WASM module
- Generates examples.js
- Deploys to `https://rask-lang.dev/app/`
