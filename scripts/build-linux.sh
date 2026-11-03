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
python3 - "$project_root" "$work" <<'PY'
from pathlib import Path
import json,sys
root,work=map(Path,sys.argv[1:])
lib=Path('/usr/lib/x86_64-linux-gnu')
files={
    '/usr/bin/ffmpeg':str(root/'target/linux-ffmpeg/bin/ffmpeg'),
    '/usr/bin/fc-match':'/usr/bin/fc-match',
    '/usr/lib/libpipewire-0.3.so.0':str((lib/'libpipewire-0.3.so.0').resolve()),
    '/usr/share/licenses/vk-fast/ffmpeg':str(root/'target/linux-ffmpeg/licenses'),
    '/usr/share/pipewire/client.conf':'/usr/share/pipewire/client.conf',
}
for name in ['protocol-native','client-node','client-device','adapter','metadata','session-manager','rt','rtkit']:
    source=lib/f'pipewire-0.3/libpipewire-module-{name}.so'
    if source.is_file():files[f'/usr/lib/pipewire-0.3/{source.name}']=str(source)
for name in ['support','audioconvert','audiomixer']:
    source=lib/'spa-0.2'/name
    files[f'/usr/lib/spa-0.2/{name}']=str(source)
config={'bundle':{'active':True,'category':'Music','shortDescription':'VK Music desktop player',
    'linux':{'appimage':{'files':files,'bundleMediaFramework':False}}}}
(work/'bundle.json').write_text(json.dumps(config))
PY
# The bundler normalizes Cargo features in its project manifest. Keep that
# bookkeeping in a disposable project instead of editing the working checkout.
bundle_project="$work/bundle-project"
mkdir -p "$bundle_project"
cp Cargo.toml Cargo.lock tauri.conf.json "$bundle_project/"
for entry in src assets webview-assets vendor .cargo build.rs; do
    ln -sfn "$project_root/$entry" "$bundle_project/$entry"
done
(
    cd "$bundle_project"
    "${VK_FAST_TAURI_CLI:-cargo-tauri}" bundle --bundles appimage --ci --no-sign --no-binary-patching --config "$work/bundle.json"
)
python3 scripts/package-linux.py --appdir "$CARGO_TARGET_DIR/release/bundle/appimage/VK Fast.AppDir" --output "$output"
