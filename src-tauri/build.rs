fn main() {
    // Unit-test harnesses are separate Windows executables and do not inherit
    // the Tauri application executable's manifest. Once code reaches Tauri's
    // dialog runtime they must activate Common Controls v6 themselves, or the
    // Windows loader cannot resolve comctl32!TaskDialogIndirect.
    //
    // MANIFESTDEPENDENCY merges this assembly into each linker's manifest. It
    // deliberately avoids MANIFESTINPUT, which would collide with the manifest
    // resource that tauri-build already embeds in the desktop binary.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:\"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='<redacted>' language='*'\""
        );
    }

    tauri_build::build()
}
