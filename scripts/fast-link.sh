#!/bin/sh
# Linker driver for local Linux (gnu) builds, wired in by .cargo/config.toml.
#
# With both mold and clang installed it links with mold, as CI's test job
# does; otherwise it is exactly the toolchain's default `cc`, so a machine
# without them builds as before. RUPU_NO_FAST_LINKER=1 opts out.
#
# `-fuse-ld=mold` goes last: on x86_64 rustc itself passes `-fuse-ld=lld`
# (its bundled rust-lld, the default since Rust 1.90), and the last
# `-fuse-ld` wins. clang rather than `cc` because GCC only learned
# `-fuse-ld=mold` in 12.1.
if [ -z "${RUPU_NO_FAST_LINKER:-}" ] \
    && command -v mold >/dev/null 2>&1 \
    && command -v clang >/dev/null 2>&1; then
    exec clang "$@" -fuse-ld=mold
fi
exec cc "$@"
