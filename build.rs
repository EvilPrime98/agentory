//! Windows only: embed the app icon and version info into the .exe.
//! Failing to do so (no `rc.exe`) must not break the build, so it only warns.

fn main() {
    println!("cargo:rerun-if-changed=packaging/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    if std::path::Path::new("packaging/icon.ico").exists() {
        res.set_icon("packaging/icon.ico");
    }
    res.set("ProductName", "Agentory");
    res.set("FileDescription", "Agentory");
    res.set("LegalCopyright", "Copyright © AminPerez. GPL-3.0-or-later");
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the Windows icon/version resources: {e}");
    }
}
