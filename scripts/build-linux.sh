#!/usr/bin/env bash
# Build on Ubuntu 24.04 (or an equivalent glibc 2.39 baseline).
set -euo pipefail
project_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"
work="${VK_FAST_LINUX_WORK:-$project_root/.build/linux-release}"
output="${VK_FAST_OUTPUT:-$project_root/dist}"
mkdir -p "$work" "$output"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$work/target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_INCREMENTAL=0
cargo build --release --locked
VK_FAST_LINUX_FFMPEG_WORK="$work/ffmpeg" ./scripts/build-ffmpeg-linux.sh
python3 scripts/package-linux.py --executable "$CARGO_TARGET_DIR/release/vk-fast" --output "$output"
