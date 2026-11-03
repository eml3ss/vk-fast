#!/usr/bin/env bash
# Minimal audio decoder for Linux bundles; never touches the Windows decoder.
set -euo pipefail
project_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
work="${VK_FAST_LINUX_FFMPEG_WORK:-$project_root/.build/linux-ffmpeg}"
output="${VK_FAST_LINUX_FFMPEG_OUTPUT:-$project_root/target/linux-ffmpeg}"
jobs="${VK_FAST_JOBS:-4}"
ffmpeg_version=9.0.2
ffmpeg_sha=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
tls_version=3.6.7
tls_sha=a7e8bcbec0e6f761b4af24f25677626b35f762f68eef79c08677a363212d11f6
case "$jobs" in ''|*[!0-9]*|0) exit 2;; esac
mkdir -p "$work/downloads" "$work/source" "$work/build" "$output/bin" "$output/licenses"
work="$(cd -- "$work" && pwd)"
output="$(cd -- "$output" && pwd)"
identity=$(cat "$project_root/scripts/build-ffmpeg-linux.sh" "$project_root"/scripts/ffmpeg-tls/*.{py,patch,pem} <(getconf GNU_LIBC_VERSION) | sha256sum | cut -d' ' -f1)
if [[ -x "$output/bin/ffmpeg" && -f "$output/build-id.txt" && "$(cat "$output/build-id.txt")" == "$identity" ]]; then
    printf '%s\n' 'Reusing cached Linux audio decoder.'
    exit 0
fi
fetch() {
    local name=$1 url=$2 checksum=$3
    if [[ ! -f "$work/downloads/$name" ]]; then
        curl --fail --location --proto '=https' --tlsv1.2 --retry 3 "$url" --output "$work/downloads/$name.part"
        mv "$work/downloads/$name.part" "$work/downloads/$name"
    fi
    printf '%s  %s\n' "$checksum" "$work/downloads/$name" | sha256sum --check --status
}
fetch "ffmpeg-$ffmpeg_version.tar.xz" "https://ffmpeg.org/releases/ffmpeg-$ffmpeg_version.tar.xz" "$ffmpeg_sha"
fetch "mbedtls-$tls_version.tar.bz2" "https://github.com/Mbed-TLS/mbedtls/releases/download/mbedtls-$tls_version/mbedtls-$tls_version.tar.bz2" "$tls_sha"
tls_source="$work/source/mbedtls-$tls_version"
if [[ ! -f "$tls_source/Makefile" ]]; then tar --no-same-owner -xjf "$work/downloads/mbedtls-$tls_version.tar.bz2" -C "$work/source"; fi
make -C "$tls_source" -j "$jobs" lib CC=gcc AR=ar 'CFLAGS=-O2 -ffunction-sections -fdata-sections'
source_dir="$work/source/ffmpeg-$ffmpeg_version"
if [[ ! -f "$source_dir/configure" ]]; then tar --no-same-owner -xJf "$work/downloads/ffmpeg-$ffmpeg_version.tar.xz" -C "$work/source"; fi
python3 "$project_root/scripts/ffmpeg-tls/prepare.py" --source "$source_dir" --download-dir "$work/downloads"
cd "$work/build"
"$source_dir/configure" --prefix="$output" --disable-autodetect --disable-everything \
    --disable-programs --enable-ffmpeg --disable-doc --disable-debug --enable-small --disable-x86asm \
    --disable-avdevice --disable-swscale --disable-shared --enable-static --enable-gpl --enable-version3 \
    --enable-network --enable-mbedtls --enable-avcodec --enable-avformat --enable-avfilter --enable-swresample \
    --enable-decoder=mp3float,mp2float,aac,aac_latm,pcm_s16le,pcm_f32le --enable-encoder=pcm_f32le \
    --enable-demuxer=mp3,aac,hls,mpegts,mov,wav --enable-muxer=pcm_f32le --enable-parser=aac,aac_latm,mpegaudio \
    --enable-protocol=file,pipe,http,https,tcp,tls,crypto --enable-filter=aformat,anull,aresample,atrim \
    "--extra-cflags=-ffunction-sections -fdata-sections -I$tls_source/include" \
    "--extra-ldflags=-Wl,--gc-sections -L$tls_source/library" \
    '--extra-libs=-lmbedtls -lmbedx509 -lmbedcrypto -pthread' > configure-summary.txt
make -j "$jobs" ffmpeg
install -m755 ffmpeg "$output/bin/ffmpeg"
strip "$output/bin/ffmpeg"
cp "$source_dir"/COPYING.* "$source_dir/LICENSE.md" "$output/licenses/"
cp "$tls_source/LICENSE" "$output/licenses/Mbed-TLS.txt"
cp "$project_root/scripts/ffmpeg-tls/LICENSE-MPL-2.0.txt" "$output/licenses/Mozilla-CA-MPL-2.0.txt"
cp configure-summary.txt "$output/BUILDINFO.txt"
printf '%s\n' "$identity" > "$output/build-id.txt"
