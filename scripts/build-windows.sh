#!/usr/bin/env bash
set -euo pipefail
project_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export XWIN_ARCH=x86_64
native_windows=0
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) native_windows=1 ;;
esac
if [[ -n "${VK_FAST_PYTHON:-}" ]]; then
    python=("$VK_FAST_PYTHON")
elif [[ "$native_windows" == 1 ]]; then
    if command -v py >/dev/null 2>&1; then
        python=(py -3)
    else
        python=(python)
    fi
else
    python=(python3)
fi
"${python[@]}" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else "Python 3.11+ is required for Windows packaging")'
if [[ "$native_windows" == 1 ]]; then
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
    cargo build --release --locked --target x86_64-pc-windows-msvc
elif [[ -n "${VK_FAST_XWIN_ROOT:-}" ]]; then
    "${python[@]}" scripts/windows-msvc.py build --release --locked
else
    export RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
    cargo xwin build --release --locked --target x86_64-pc-windows-msvc
fi
"${python[@]}" scripts/collect-licenses.py
"${python[@]}" scripts/package-windows.py "$@"
