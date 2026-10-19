#!/bin/sh
# rustc wrapper that automatically enables sccache if installed,
# or transparently falls back to direct rustc execution.
if command -v sccache >/dev/null 2>&1; then
    # Enforce 5GB cache size limit by default unless overridden by environment
    export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-5G}"
    exec sccache "$@"
else
    exec "$@"
fi
