# Playground

Try Rask directly in your browser with our interactive playground!

**[Launch Playground →](/rask/playground/)**

## Features

The playground provides:
- ✨ **Online editor** with syntax highlighting
- ⚡ **Instant execution** - no setup required
- 🔗 **Shareable code** snippets via URL
- 📚 **Example programs** to explore
- 🎮 **Quick experimentation** without installation

## How to Use

1. **Write code** in the left editor pane
2. **Click "Run"** or press `Ctrl+Enter` to execute
3. **View output** in the right pane
4. **Load examples** from the dropdown menu
5. **Share your code** with the "Share" button

## Limitations

The browser-based playground has some limitations compared to local execution:

- ❌ **No file I/O** - `fs` module is disabled
- ❌ **No networking** - `net` module is disabled
- ❌ **No stdin** - interactive input not supported
- ✅ **Most features work** - math, collections, json, pattern matching, etc.

## Try These Examples

Click the examples dropdown in the playground to try:
- **Hello World** - Basic println and output
- **Collections** - Working with Vec, structs, and pattern matching
- **Pattern Matching** - Demonstrating match expressions
- **Math Demo** - Mathematical operations and calculations

## Local Development

For full language features including file I/O and networking:
1. [Install Rask locally](../getting-started/installation.md)
2. Run the [examples](../examples/README.md)
3. Build real applications

## Technical Details

The playground compiles the Rask interpreter to WebAssembly using `wasm-pack`. Code executes entirely in your browser with no server-side processing.

**WASM bundle size:** ~200KB gzipped
**Supported browsers:** Chrome, Firefox, Safari (latest versions)

## Source Code

The playground is open source:
- [Playground UI](https://github.com/dritory/rask/tree/main/docs/playground)
- [WASM bindings](https://github.com/dritory/rask/tree/main/compiler/crates/rask-wasm)

## Feedback

Found a bug or have a suggestion? [Open an issue](https://github.com/dritory/rask/issues) on GitHub!
