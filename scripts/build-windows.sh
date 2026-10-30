#!/usr/bin/env bash
set -euo pipefail
project_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export XWIN_ARCH=x86_64
decoder_backend=mbedtls-3.6.7-mozilla-2026-10-20-v1
if [[ "${VK_FAST_REBUILD_FFMPEG:-0}" == "1" || ! -f target/windows-ffmpeg/bin/ffmpeg.exe || ! -f target/windows-ffmpeg/TLS-BACKEND.txt ]] || [[ "$(cat target/windows-ffmpeg/TLS-BACKEND.txt 2>/dev/null)" != "$decoder_backend" ]]; then
    ./scripts/build-ffmpeg-windows.sh
else
    printf '%s\n' 'Reusing target/windows-ffmpeg/bin/ffmpeg.exe'
fi
if [[ -n "${VK_FAST_XWIN_ROOT:-}" ]]; then
    python3 scripts/windows-msvc.py build --release --locked
else
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
    cargo xwin build --release --locked --target x86_64-pc-windows-msvc
fi
python3 scripts/collect-licenses.py
python3 scripts/package-windows.py "$@"
