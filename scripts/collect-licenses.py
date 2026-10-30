#!/usr/bin/env python3
"""Collect notices shipped by the Windows dependency graph fixed in Cargo.lock."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
metadata = json.loads(subprocess.check_output([
    "cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc",
], cwd=root))
active = {node["id"] for node in metadata["resolve"]["nodes"]}
output = []
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["id"] not in active or package["name"] == "vk-fast":
        continue
    directory = Path(package["manifest_path"]).parent
    output += ["=" * 72, f"{package['name']} {package['version']}",
               f"License: {package.get('license') or 'see files'}",
               "Source: " + (package.get("repository") or f"https://crates.io/crates/{package['name']}/{package['version']}"), ""]
    files = set()
    if package.get("license_file"):
        files.add(directory / package["license_file"])
    for path in directory.iterdir():
        if path.is_file() and any(path.name.upper().startswith(prefix) for prefix in
                                 ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT")):
            files.add(path)
    for path in sorted(files):
        if path.is_file() and path.stat().st_size < 1024 * 1024:
            output += ["--- " + path.name + " ---", path.read_text(errors="replace"), ""]
destination = root / "target/windows-third-party-licenses.txt"
destination.parent.mkdir(parents=True, exist_ok=True)
destination.write_text("\n".join(output), encoding="utf-8")
print(destination)
