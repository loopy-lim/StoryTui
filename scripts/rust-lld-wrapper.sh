#!/bin/sh
# Wrapper to invoke rustc's built-in rust-lld (ld64.lld) on macOS for fast linking.
# Dynamically resolves sysroot and host target triple so it works across toolchain updates.
SYSROOT=$(rustc --print sysroot)
HOST=$(rustc -vV | sed -n 's/^host: //p')
GCC_LD="$SYSROOT/lib/rustlib/$HOST/bin/gcc-ld"

if [ -d "$GCC_LD" ]; then
    exec /usr/bin/clang -B"$GCC_LD" -fuse-ld=lld -Wl,-w "$@"
else
    exec /usr/bin/clang "$@"
fi
