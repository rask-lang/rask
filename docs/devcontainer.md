# Dev container

The native backend has no Windows runtime, so on Windows Rask runs in a Linux
container. `.devcontainer/` sets one up: Rust, gcc, clang, gdb and Node, with
`rask` and `rask-lsp` built on creation and on `PATH`, and the editor extension
installed.

## Open it

Needs Docker Desktop and VS Code's Dev Containers extension.

Run **Dev Containers: Clone Repository in Container Volume** and give it the
repo URL. Don't open a Windows checkout instead: a bind mount from `C:\` makes
cargo crawl, and `core.autocrlf` leaves the shell scripts with CRLF endings
that bash rejects.

The first start builds the compiler, which takes a few minutes. After pulling
compiler changes, rebuild it with `cd compiler && cargo build --release -p rask-cli -p rask-lsp`.

## What survives a rebuild

Three named volumes outlive the container:

| Volume | Mounted at | Holds |
|---|---|---|
| `rask-projects` | `/workspaces/projects` | Your own Rask projects, kept outside the compiler repo |
| `rask-claude` | `/home/vscode/.claude` | Claude Code login, settings and memory |
| `rask-cargo-registry` | `/usr/local/cargo/registry` | Downloaded crates |

Open a project with **File > Open Folder** → `/workspaces/projects/<name>`; it
stays in the same container.
