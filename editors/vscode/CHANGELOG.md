# Changelog

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
