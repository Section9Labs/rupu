# Linux build image for the rupu CLI.
#
# Produces a fully static musl binary: the `rust:*-alpine` images use
# `*-unknown-linux-musl` as the host target, so a plain `cargo build`
# inside this image is already static — no `--target` flag and no
# `-C target-feature=+crt-static` needed.
#
# Static linking is the point. A glibc build made on ubuntu-latest
# (glibc 2.39) silently refuses to run on Debian 12, Ubuntu 22.04,
# RHEL 9, or Amazon Linux 2. This binary runs on all of them, plus
# Alpine, distroless, and any WSL2 distribution.
#
# Built natively on each architecture — linux/amd64 on ubuntu-latest,
# linux/arm64 on ubuntu-24.04-arm — so there is no emulation anywhere.
#
# The Rust version MUST track rust-toolchain.toml.
FROM rust:1.95-alpine3.21

# musl-dev, gcc, g++: the musl C toolchain that vendored libgit2 and
#   aws-lc-rs compile against.
# perl, make: required by git2's `vendored-openssl` feature.
# cmake, clang, clang-dev: required by aws-lc-rs, which rupu-cli installs
#   as the process-level rustls provider (crates/rupu-cli/src/main.rs).
# pkgconf: consulted by several -sys build scripts.
# git: build scripts and several tests shell out to it.
# file: used by CI to prove the built binary is statically linked. Alpine's
#   base image does not ship it.
# ripgrep: rupu's `grep` tool shells out to `rg` and hard-errors without it
#   (crates/rupu-tools/src/grep.rs) — it is a genuine runtime dependency of
#   the product, not just of the tests. `ast-grep` is the other external
#   binary rupu can use, but that one is gracefully optional (its tests skip
#   when absent), so it is deliberately not installed here.
# bash: test fixtures write `#!/bin/sh` scripts, but a real shell makes
#   debugging a failing build far easier.
# nodejs, npm: `make cp-web` builds the embedded CP UI before a release.
#
# Deliberately NOT installed: dbus-dev. `keyring` was removed from the
# workspace (PR #554) precisely so the Linux graph has no D-Bus
# dependency. If a build here ever asks for it, something reintroduced
# `keyring` — fix that, do not add the package.
RUN apk add --no-cache \
      musl-dev gcc g++ make perl cmake clang clang-dev \
      pkgconf git file ripgrep bash nodejs npm

ENV CARGO_TERM_COLOR=always
WORKDIR /work
