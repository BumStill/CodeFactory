fn main() {
    // Unit-test harnesses are separate Windows executables and do not inherit
    // the Tauri application executable's manifest. Once code reaches Tauri's
    // dialog runtime they must activate Common Controls v6 themselves, or the
    // Windows loader cannot resolve comctl32!TaskDialogIndirect.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-test-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }

    tauri_build::build()
}
