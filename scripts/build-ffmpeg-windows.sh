#!/usr/bin/env bash
# Build the portable audio decoder from authenticated, pinned upstream sources.
set -euo pipefail

VK_FAST_RECIPE_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
VK_FAST_RECIPE_PATH="$VK_FAST_RECIPE_DIR/$(basename -- "${BASH_SOURCE[0]}")"
VK_FAST_PROJECT_ROOT=$(cd -- "$VK_FAST_RECIPE_DIR/.." && pwd)
VK_FAST_FFMPEG_WORK=${VK_FAST_FFMPEG_WORK:-"$VK_FAST_PROJECT_ROOT/.build/windows-ffmpeg"}
VK_FAST_FFMPEG_OUTPUT=${VK_FAST_FFMPEG_OUTPUT:-"$VK_FAST_PROJECT_ROOT/target/windows-ffmpeg"}
VK_FAST_FFMPEG_VERSION=9.0.2
VK_FAST_FFMPEG_ARCHIVE=ffmpeg-$VK_FAST_FFMPEG_VERSION.tar.xz
VK_FAST_FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
VK_FAST_LLVM_VERSION=20261020
VK_FAST_LLVM_NAME=llvm-mingw-$VK_FAST_LLVM_VERSION-ucrt-ubuntu-22.04-x86_64
VK_FAST_LLVM_SHA256=55064b60b991a1e1e8090b8c2f84e82b22cfe5903d12c159d65064cf5e8ae335
VK_FAST_MBEDTLS_VERSION=3.6.7
VK_FAST_MBEDTLS_ARCHIVE=mbedtls-$VK_FAST_MBEDTLS_VERSION.tar.bz2
VK_FAST_MBEDTLS_SHA256=a7e8bcbec0e6f761b4af24f25677626b35f762f68eef79c08677a363212d11f6
VK_FAST_TLS_BACKEND=mbedtls-3.6.7-mozilla-2026-10-20-v1
VK_FAST_TLS_SUPPORT="$VK_FAST_RECIPE_DIR/ffmpeg-tls"
VK_FAST_JOBS=${VK_FAST_JOBS:-4}
case "$VK_FAST_JOBS" in ''|*[!0-9]*|0) printf '%s\n' 'VK_FAST_JOBS must be a positive integer.' >&2; exit 2;; esac

for command in curl sha256sum tar make gcc python3 patch; do
    command -v "$command" >/dev/null || { printf 'Missing build tool: %s\n' "$command" >&2; exit 2; }
done
mkdir -p "$VK_FAST_FFMPEG_WORK" "$VK_FAST_FFMPEG_OUTPUT"
VK_FAST_FFMPEG_WORK=$(cd -- "$VK_FAST_FFMPEG_WORK" && pwd)
VK_FAST_FFMPEG_OUTPUT=$(cd -- "$VK_FAST_FFMPEG_OUTPUT" && pwd)
VK_FAST_DOWNLOADS="$VK_FAST_FFMPEG_WORK/downloads"
mkdir -p "$VK_FAST_DOWNLOADS" "$VK_FAST_FFMPEG_WORK/tools" "$VK_FAST_FFMPEG_WORK/source" "$VK_FAST_FFMPEG_WORK/build"

fetch_checked() {
    local url=$1 name=$2 checksum=$3
    if [[ ! -f "$VK_FAST_DOWNLOADS/$name" ]]; then
        curl --fail --location --proto '=https' --tlsv1.2 --retry 3 --output "$VK_FAST_DOWNLOADS/$name.part" "$url"
        mv -- "$VK_FAST_DOWNLOADS/$name.part" "$VK_FAST_DOWNLOADS/$name"
    fi
    printf '%s  %s\n' "$checksum" "$VK_FAST_DOWNLOADS/$name" | sha256sum --check --status || {
        printf 'Checksum mismatch: %s\n' "$name" >&2; exit 1;
    }
}
fetch_checked "https://ffmpeg.org/releases/$VK_FAST_FFMPEG_ARCHIVE" "$VK_FAST_FFMPEG_ARCHIVE" "$VK_FAST_FFMPEG_SHA256"
fetch_checked "https://github.com/mstorsjo/llvm-mingw/releases/download/$VK_FAST_LLVM_VERSION/$VK_FAST_LLVM_NAME.tar.xz" "$VK_FAST_LLVM_NAME.tar.xz" "$VK_FAST_LLVM_SHA256"
fetch_checked "https://github.com/Mbed-TLS/mbedtls/releases/download/mbedtls-$VK_FAST_MBEDTLS_VERSION/$VK_FAST_MBEDTLS_ARCHIVE" "$VK_FAST_MBEDTLS_ARCHIVE" "$VK_FAST_MBEDTLS_SHA256"
if [[ ! -x "$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/bin/x86_64-w64-mingw32-clang" ]]; then
    tar -xJf "$VK_FAST_DOWNLOADS/$VK_FAST_LLVM_NAME.tar.xz" -C "$VK_FAST_FFMPEG_WORK/tools"
fi
export PATH="$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/bin:$PATH"
# Cache the static TLS libraries separately from FFmpeg and application builds.
VK_FAST_MBEDTLS_SOURCE="$VK_FAST_FFMPEG_WORK/source/mbedtls-$VK_FAST_MBEDTLS_VERSION"
VK_FAST_MBEDTLS_PREFIX="$VK_FAST_FFMPEG_WORK/tools/mbedtls-$VK_FAST_MBEDTLS_VERSION-x86_64"
VK_FAST_MBEDTLS_BUILD_ID=$(printf '%s\n' "$VK_FAST_MBEDTLS_SHA256" "$VK_FAST_LLVM_SHA256" '-Oz -ffunction-sections -fdata-sections; WINDOWS_BUILD=1' | sha256sum | cut -d ' ' -f 1)
if [[ ! -f "$VK_FAST_MBEDTLS_SOURCE/Makefile" ]]; then
    tar -xjf "$VK_FAST_DOWNLOADS/$VK_FAST_MBEDTLS_ARCHIVE" -C "$VK_FAST_FFMPEG_WORK/source"
fi
if [[ ! -f "$VK_FAST_MBEDTLS_PREFIX/lib/libmbedtls.a" || ! -f "$VK_FAST_MBEDTLS_PREFIX/lib/libmbedx509.a" || ! -f "$VK_FAST_MBEDTLS_PREFIX/lib/libmbedcrypto.a" || ! -f "$VK_FAST_MBEDTLS_PREFIX/build-id.txt" ]] || [[ "$(cat "$VK_FAST_MBEDTLS_PREFIX/build-id.txt" 2>/dev/null)" != "$VK_FAST_MBEDTLS_BUILD_ID" ]]; then
    make -C "$VK_FAST_MBEDTLS_SOURCE/library" clean
    make -C "$VK_FAST_MBEDTLS_SOURCE" -j "$VK_FAST_JOBS" lib \
        WINDOWS_BUILD=1 CC=x86_64-w64-mingw32-clang AR=llvm-ar AR_DASH= \
        'CFLAGS=-Oz -ffunction-sections -fdata-sections'
    mkdir -p "$VK_FAST_MBEDTLS_PREFIX/include" "$VK_FAST_MBEDTLS_PREFIX/lib"
    cp -R -- "$VK_FAST_MBEDTLS_SOURCE/include/mbedtls" "$VK_FAST_MBEDTLS_SOURCE/include/psa" "$VK_FAST_MBEDTLS_PREFIX/include/"
    cp -- "$VK_FAST_MBEDTLS_SOURCE"/library/libmbed{tls,x509,crypto}.a "$VK_FAST_MBEDTLS_PREFIX/lib/"
    printf '%s\n' "$VK_FAST_MBEDTLS_BUILD_ID" > "$VK_FAST_MBEDTLS_PREFIX/build-id.txt"
else
    printf '%s\n' 'Reusing cached static Mbed TLS libraries.'
fi
# A dedicated source/build tree keeps the previous portable decoder intact.
# Include the trust helper, patch and PEM in the source identity, so a changed
# trust input cannot silently reuse a differently patched tree.
VK_FAST_TRUST_ID=$(python3 - "$VK_FAST_TLS_SUPPORT" <<'PY'
import hashlib
from pathlib import Path
import sys
root = Path(sys.argv[1])
digest = hashlib.sha256()
for pattern in ("prepare.py", "*.patch", "*.pem"):
    for path in sorted(root.glob(pattern)):
        digest.update(path.name.encode())
        digest.update(path.read_bytes())
print(digest.hexdigest()[:16])
PY
)
VK_FAST_FFMPEG_SOURCE="$VK_FAST_FFMPEG_WORK/source/ffmpeg-$VK_FAST_FFMPEG_VERSION-$VK_FAST_TLS_BACKEND-$VK_FAST_TRUST_ID"
VK_FAST_FFMPEG_BUILD="$VK_FAST_FFMPEG_WORK/build-$VK_FAST_TLS_BACKEND-$VK_FAST_TRUST_ID"
mkdir -p "$VK_FAST_FFMPEG_SOURCE" "$VK_FAST_FFMPEG_BUILD"
if [[ ! -f "$VK_FAST_FFMPEG_SOURCE/configure" ]]; then
    tar -xJf "$VK_FAST_DOWNLOADS/$VK_FAST_FFMPEG_ARCHIVE" --strip-components=1 -C "$VK_FAST_FFMPEG_SOURCE"
fi
python3 "$VK_FAST_TLS_SUPPORT/prepare.py" --source "$VK_FAST_FFMPEG_SOURCE" --download-dir "$VK_FAST_DOWNLOADS"
# The CLI selects several small common filters and the HLS demuxer selects its
# container helpers. No video decoder, video encoder or hardware API is enabled.
VK_FAST_CONFIGURE=(
    "$VK_FAST_FFMPEG_SOURCE/configure"
    "--prefix=$VK_FAST_FFMPEG_OUTPUT"
    --target-os=mingw32 --arch=x86_64 --enable-cross-compile
    --cross-prefix=x86_64-w64-mingw32-
    --cc=x86_64-w64-mingw32-clang --cxx=x86_64-w64-mingw32-clang++
    --ar=llvm-ar --ranlib=llvm-ranlib --nm=llvm-nm --strip=llvm-strip
    --disable-autodetect --disable-everything --disable-programs --enable-ffmpeg
    --disable-doc --disable-debug --enable-small --disable-x86asm
    --disable-avdevice --disable-swscale --disable-shared --enable-static
    --enable-gpl --enable-version3 --enable-network --disable-schannel --enable-mbedtls
    --enable-avcodec --enable-avformat --enable-avfilter --enable-swresample
    --enable-decoder=mp3float,mp2float,aac,aac_latm,pcm_s16le,pcm_f32le
    --enable-encoder=pcm_f32le
    --enable-demuxer=mp3,aac,hls,mpegts,mov,wav
    --enable-muxer=pcm_f32le
    --enable-parser=aac,aac_latm,mpegaudio
    --enable-protocol=file,pipe,http,https,tcp,tls,crypto
    --enable-filter=aformat,anull,aresample,atrim
    "--extra-cflags=-ffunction-sections -fdata-sections -I$VK_FAST_MBEDTLS_PREFIX/include"
    "--extra-ldflags=-static -Wl,--gc-sections -Wl,--no-insert-timestamp -L$VK_FAST_MBEDTLS_PREFIX/lib"
    '--extra-libs=-lmbedtls -lmbedx509 -lmbedcrypto -lbcrypt -lws2_32'
)
cd -- "$VK_FAST_FFMPEG_BUILD"
"${VK_FAST_CONFIGURE[@]}" > configure-summary.txt
make -j "$VK_FAST_JOBS" ffmpeg.exe
mkdir -p "$VK_FAST_FFMPEG_OUTPUT/bin" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg" "$VK_FAST_FFMPEG_OUTPUT/source"
cp -- ffmpeg.exe "$VK_FAST_FFMPEG_OUTPUT/bin/ffmpeg.exe"
llvm-strip "$VK_FAST_FFMPEG_OUTPUT/bin/ffmpeg.exe"
cp -- "$VK_FAST_FFMPEG_SOURCE"/COPYING.* "$VK_FAST_FFMPEG_SOURCE/LICENSE.md" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg/"
cp -- "$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/LICENSE.TXT" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg/LLVM-MinGW-LICENSE.TXT"
cp -- "$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/x86_64-w64-mingw32/share/mingw32/COPYING" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg/MinGW-COPYING.txt"
cp -- "$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/x86_64-w64-mingw32/share/mingw32/COPYING.MinGW-w64-runtime.txt" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg/"
cp -- "$VK_FAST_FFMPEG_WORK/tools/$VK_FAST_LLVM_NAME/x86_64-w64-mingw32/share/mingw32/COPYING.MinGW-w64.txt" "$VK_FAST_FFMPEG_OUTPUT/licenses/ffmpeg/"
cp -- "$VK_FAST_DOWNLOADS/$VK_FAST_FFMPEG_ARCHIVE" "$VK_FAST_FFMPEG_OUTPUT/source/"
cp -- "$VK_FAST_RECIPE_PATH" "$VK_FAST_FFMPEG_OUTPUT/source/"
cp -- "$VK_FAST_DOWNLOADS/$VK_FAST_MBEDTLS_ARCHIVE" "$VK_FAST_FFMPEG_OUTPUT/source/"
mkdir -p "$VK_FAST_FFMPEG_OUTPUT/licenses/mbedtls" "$VK_FAST_FFMPEG_OUTPUT/source/ffmpeg-tls"
cp -- "$VK_FAST_MBEDTLS_SOURCE/LICENSE" "$VK_FAST_FFMPEG_OUTPUT/licenses/mbedtls/"
cp -R -- "$VK_FAST_TLS_SUPPORT/." "$VK_FAST_FFMPEG_OUTPUT/source/ffmpeg-tls/"
cp -- configure-summary.txt "$VK_FAST_FFMPEG_OUTPUT/source/ffmpeg-configure-summary.txt"
{
    printf 'FFmpeg version: %s\nFFmpeg SHA256: %s\nLLVM-MinGW release: %s\nLLVM-MinGW SHA256: %s\n' "$VK_FAST_FFMPEG_VERSION" "$VK_FAST_FFMPEG_SHA256" "$VK_FAST_LLVM_VERSION" "$VK_FAST_LLVM_SHA256"
    printf 'TLS backend: %s\nMbed TLS SHA256: %s\nStatic TLS cache: %s\nTrust input: %s\n' "$VK_FAST_TLS_BACKEND" "$VK_FAST_MBEDTLS_SHA256" "$VK_FAST_MBEDTLS_BUILD_ID" "$VK_FAST_TRUST_ID"
    printf 'Configure command: '
    printf '%q ' "${VK_FAST_CONFIGURE[@]}"
    printf '\n'
    x86_64-w64-mingw32-clang --version
} > "$VK_FAST_FFMPEG_OUTPUT/source/BUILDINFO.txt"
# Only a completed build can satisfy the normal Windows release cache gate.
printf '%s\n' "$VK_FAST_TLS_BACKEND" > "$VK_FAST_FFMPEG_OUTPUT/TLS-BACKEND.txt"
sha256sum "$VK_FAST_FFMPEG_OUTPUT/bin/ffmpeg.exe"
