# Changelog

## Unreleased

- Highlight every keyword the compiler has (`let`, `private`, `union`, `catch`, `discard`, `extern`, `select_priority`, `void` were missing); drop `deliver`, `own` and `step`, which aren't keywords
- String interpolation colors as code: `"{respond(mutate world, words)}"` scopes its `{ }` hole as an expression, with nested brackets, strings and `:spec` formats; `{{`/`}}` are literal braces
- Raw (`r"…"`, `r#"…"#`), triple-quoted and `x"…"` strings

## 0.2.1

- Update for `let` → `mut` rename (mutable bindings)
- Update for optionals redesign: `none` sentinel (lowercase), no `Some`/`None`/`Ok`/`Err` constructors
- Refresh snippets to use `if x? { … }` / `if x? as v { … }` narrowing

## 0.2.0

- Modern LSP client integration
- Status bar indicator for server state
- Better startup error reporting (output channel + quick actions)
- Commands: `rask check`, `rask test`, `rask build`
- Settings: `rask.cliPath`, `rask.inlayHints.enable`

## 0.1.0

- Syntax highlighting for `.rk` files
- Rask code blocks in Markdown
- Language Server Protocol client
- File icon for `.rk` files
