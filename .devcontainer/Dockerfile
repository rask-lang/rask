# SPDX-License-Identifier: (MIT OR Apache-2.0)
FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# gcc ships with the base image; clang is the other compiler CI checks the
# runtime with, gdb is what the debug-native skill reaches for.
RUN apt-get update \
 && apt-get install -y --no-install-recommends clang gdb \
 && rm -rf /var/lib/apt/lists/*
