# Playground Development

This directory will contain the WASM-based playground implementation.

## Planned Architecture

- **Frontend**: HTML/JS editor (CodeMirror or Monaco)
- **Backend**: Rask interpreter compiled to WASM
- **Features**: Syntax highlighting, instant execution, sharing

## Development TODO

- [ ] Compile interpreter to WASM
- [ ] Create editor UI
- [ ] Implement syntax highlighting for Rask
- [ ] Add example programs
- [ ] Implement URL sharing
- [ ] Deploy as part of docs site

## Integration

Once built, the playground will be embedded in the book at `docs/book/src/playground/`.

## Technical Notes

### WASM Compilation

The interpreter needs to be compiled to WASM. This requires:
- Ensuring all dependencies are WASM-compatible
- Adding wasm-bindgen bindings
- Building with `wasm-pack`

### Editor

Options:
- CodeMirror 6 (lightweight, good Rust support)
- Monaco (VS Code editor, heavier but feature-rich)

### Deployment

Deploy alongside mdBook output in GitHub Pages.
