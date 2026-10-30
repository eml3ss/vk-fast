#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
binary=$(realpath -- "${1:-$project_dir/target/debug/vk-fast}")
if [[ ! -x "$binary" || "$binary" == *$'\n'* || "$binary" == *$'\r'* ]]; then
    echo 'VK Fast executable is unavailable.' >&2
    exit 1
fi

applications="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
mkdir -p -- "$applications"
escaped=${binary//\\/\\\\}
escaped=${escaped//\"/\\\"}
escaped=${escaped//%/%%}
cat > "$applications/vk-fast-auth.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=VK Fast authorization
Exec="$escaped" --auth-callback %u
Icon=vk-fast
Terminal=false
NoDisplay=true
MimeType=x-scheme-handler/vkfast;
EOF
update-desktop-database "$applications"
xdg-mime default vk-fast-auth.desktop x-scheme-handler/vkfast
echo 'Registered vkfast application links for the current user.'
