// SPDX-License-Identifier: GPL-3.0-only

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/vk-fast-icon.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Explorer reads the executable's icon and details from its PE resources,
    // independently of the live window icon that egui sets.
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    let numeric = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0).to_string())
        .chain(std::iter::repeat_n("0".to_owned(), 4))
        .take(4)
        .collect::<Vec<_>>()
        .join(",");
    let icon = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("assets/vk-fast-icon.ico");
    let icon = icon.display().to_string().replace('\\', "\\\\");
    let resource = format!(
        r#"#pragma code_page(65001)
32512 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
{{
  BLOCK "StringFileInfo"
  {{
    BLOCK "040904B0"
    {{
      VALUE "CompanyName", "eml3ss"
      VALUE "FileDescription", "VK Fast"
      VALUE "FileVersion", "{version}"
      VALUE "ProductName", "VK Fast"
      VALUE "ProductVersion", "{version}"
      VALUE "OriginalFilename", "vk-fast.exe"
    }}
  }}
  BLOCK "VarFileInfo"
  {{
    VALUE "Translation", 0x0409, 1200
  }}
}}
"#
    );
    let path =
        PathBuf::from(env::var("OUT_DIR").expect("build output directory")).join("vk-fast.rc");
    fs::write(&path, resource).expect("write Windows resource script");
    embed_resource::compile(&path, embed_resource::NONE)
        .manifest_required()
        .expect("compile Windows resources");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Portable builds must run without a separately installed VC++
        // runtime: link it statically and use Windows' built-in UCRT.
        static_vcruntime::metabuild();
        println!("cargo:rustc-link-lib=ucrt");
    }
}
