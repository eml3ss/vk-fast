#!/usr/bin/env python3
"""Run Cargo for Windows x86-64 against an xwin SDK with host LLVM tools."""
import os
from pathlib import Path
import shlex
import subprocess
import sys

TARGET = "x86_64-pc-windows-msvc"


def main():
    sdk = Path(os.environ["VK_FAST_XWIN_ROOT"]).resolve()
    includes = [sdk / p for p in ("crt/include", "sdk/include/ucrt", "sdk/include/um", "sdk/include/shared", "sdk/include/winrt")]
    libraries = [sdk / p for p in ("crt/lib/x86_64", "sdk/lib/um/x86_64", "sdk/lib/ucrt/x86_64")]
    if not all(path.is_dir() for path in includes + libraries):
        raise SystemExit("VK_FAST_XWIN_ROOT must contain the x86-64 CRT and Windows SDK exported by xwin.")
    env = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS", None)
    key = TARGET.replace("-", "_")
    cflags = ["--target=" + TARGET, "-Wno-unused-command-line-argument", "-fuse-ld=lld-link"]
    for path in includes:
        cflags.extend(["/imsvc", str(path)])
    rustflags = ["-C", "target-feature=+crt-static", "-C", "linker-flavor=lld-link", "-C", "link-arg=-nodefaultlib:ucrt", "-C", "link-arg=-defaultlib:libucrt"]
    rustflags += ["-Lnative=" + str(path) for path in libraries]
    if sysroot := env.get("VK_FAST_WINDOWS_SYSROOT"):
        rustflags.insert(0, "--sysroot=" + str(Path(sysroot).resolve()))
    env.update({
        "CC_" + key: "clang-cl", "CXX_" + key: "clang-cl", "AR_" + key: "llvm-lib",
        "CC_SHELL_ESCAPED_FLAGS": "1", "CFLAGS_" + key: shlex.join(cflags),
        "CXXFLAGS_" + key: shlex.join(cflags + ["/EHsc"]),
        "LIB": ";".join(map(str, libraries)),
        "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER": "lld-link",
        "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(rustflags),
        "RC": "llvm-rc", "RCFLAGS": shlex.join(["-I" + str(path) for path in includes]),
    })
    env.setdefault("CARGO_BUILD_JOBS", "2")
    arguments = sys.argv[1:] or ["build", "--release", "--locked"]
    if any(arg.startswith("--target") for arg in arguments):
        raise SystemExit("This script builds only Windows x86-64; do not pass another target.")
    return subprocess.run(["cargo", *arguments, "--target", TARGET], env=env).returncode


if __name__ == "__main__":
    raise SystemExit(main())
