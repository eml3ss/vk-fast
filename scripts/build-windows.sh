#!/usr/bin/env bash
set -euo pipefail
project_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export XWIN_ARCH=x86_64
if [[ -n "${VK_FAST_XWIN_ROOT:-}" ]]; then
    python3 scripts/windows-msvc.py build --release --locked
else
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
    cargo xwin build --release --locked --target x86_64-pc-windows-msvc
fi
python3 scripts/collect-licenses.py
python3 scripts/package-windows.py "$@"
