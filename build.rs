// SPDX-License-Identifier: GPL-3.0-only

fn main() {
    println!("cargo:rerun-if-changed=webview-assets");
    println!("cargo:rerun-if-changed=assets/vk-fast-icon.png");
    println!("cargo:rerun-if-changed=assets/vk-fast-icon.ico");
    // Explorer reads the PE resource, independently of egui's live window
    // icon. Keep both the executable resource and Tauri's default icon on the
    // same canonical artwork rather than the old login-webview placeholder.
    let windows = tauri_build::WindowsAttributes::new().window_icon_path("assets/vk-fast-icon.ico");
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("could not prepare the VK Fast login webview");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // Tauri links the VC runtime statically and selects Windows' built-in
        // UCRT. Make that library explicit so cross-linker default-library
        // exclusions cannot suppress both the static and system UCRT.
        println!("cargo:rustc-link-lib=ucrt");
    }
}
