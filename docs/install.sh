#!/bin/sh
# SPDX-License-Identifier: (MIT OR Apache-2.0)
#
# Installs the rask compiler from the latest GitHub release.
#
#     curl -fsSL https://rask-lang.dev/install.sh | sh
#
# It downloads the one binary for this machine into ~/.local/bin and tells you
# what's missing: a PATH entry, or the C compiler that `rask run` links with.
# Nothing else on the system is touched, so uninstalling is deleting the file.
#
# Settings, all optional:
#
#     RASK_VERSION=0.6.0      a release other than the latest
#     RASK_INSTALL_DIR=DIR    somewhere other than ~/.local/bin
#     RASK_DOWNLOAD_URL=URL   fetch the binary from here instead of GitHub,
#                             for a build under test (a file:// URL works)
#
# Served from the site root by .github/workflows/docs.yml; the copy in the
# repository is the source.

set -eu

REPO="https://github.com/rask-lang/rask"

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

# One name per binary the release workflow builds (.github/workflows/release.yml).
os="$(uname -s)"
arch="$(uname -m)"
# A terminal running under Rosetta reports x86_64 on an Apple silicon Mac,
# which runs the arm64 binary fine.
if [ "$os-$arch" = Darwin-x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
    arch=arm64
fi
case "$os-$arch" in
    Linux-x86_64 | Linux-amd64) asset=rask-linux-x86_64 ;;
    Darwin-arm64 | Darwin-aarch64) asset=rask-macos-aarch64 ;;
    *)
        fail "there is no prebuilt rask for $os on $arch.
Releases have binaries for Linux on x86_64 and macOS on Apple silicon.
On anything else, build it from source (needs Rust and a C compiler):

    git clone $REPO
    cd rask/compiler
    cargo build --release -p rask-cli

The binary is compiler/target/release/rask."
        ;;
esac

if [ -n "${RASK_DOWNLOAD_URL:-}" ]; then
    url="$RASK_DOWNLOAD_URL"
elif [ -n "${RASK_VERSION:-}" ]; then
    url="$REPO/releases/download/v${RASK_VERSION#v}/$asset"
else
    url="$REPO/releases/latest/download/$asset"
fi

dir="${RASK_INSTALL_DIR:-$HOME/.local/bin}"
command -v curl >/dev/null 2>&1 || fail "curl is needed to download rask"
mkdir -p "$dir" || fail "can't create $dir"

say "Downloading $url"
# Into a side file first, so a failed download never leaves half a binary
# where a working one was.
if ! curl -fSL --progress-bar -o "$dir/rask.part" "$url"; then
    rm -f "$dir/rask.part"
    fail "the download failed. Check the address above in a browser; if a
version was given with RASK_VERSION, that release may not exist."
fi
chmod +x "$dir/rask.part"
if ! version="$("$dir/rask.part" --version 2>&1)"; then
    rm -f "$dir/rask.part"
    fail "the downloaded file doesn't run on this machine: $version"
fi
mv "$dir/rask.part" "$dir/rask"
say "Installed $version to $dir/rask"

# `rask run`, `build` and `test` compile to a native program, and the last step
# of that is linking with the system C compiler. The interpreter
# (`rask run --interp`) needs none.
have_cc=no
if [ -n "${CC:-}" ] && command -v "$CC" >/dev/null 2>&1; then
    have_cc=yes
elif [ "$os" = Darwin ]; then
    # macOS ships a `cc` that only offers to install the developer tools, so
    # ask for the tools themselves.
    xcode-select -p >/dev/null 2>&1 && have_cc=yes
elif command -v cc >/dev/null 2>&1; then
    have_cc=yes
fi
if [ "$have_cc" = no ]; then
    say ""
    say "No C compiler found. rask links the programs it builds with one, so"
    say "until there is one only \`rask run --interp\` works. To get one:"
    if [ "$os" = Darwin ]; then
        say "    xcode-select --install"
    else
        say "    sudo apt install gcc      (Debian, Ubuntu)"
        say "    sudo dnf install gcc      (Fedora)"
    fi
fi

case ":$PATH:" in
    *":$dir:"*)
        say ""
        say "Try it: rask --version"
        ;;
    *)
        say ""
        say "$dir isn't on your PATH yet. Add this line to your shell's startup"
        say "file (~/.bashrc, ~/.zshrc, ...), then open a new terminal:"
        say ""
        case "$dir" in
            "$HOME"/*) shown="\$HOME${dir#"$HOME"}" ;;
            *) shown="$dir" ;;
        esac
        say "    export PATH=\"$shown:\$PATH\""
        ;;
esac
