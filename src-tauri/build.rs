fn main() {
    tauri_build::build();
    // Tauri embeds its application manifest in the normal executable, not
    // Cargo's lib/integration test executables. Real WebView tests link Tao's
    // SetWindowSubclass/DefSubclassProc imports, which require comctl32 v6.
    // Without this dependency the loader exits before the first test runs
    // (STATUS_ENTRYPOINT_NOT_FOUND), so a runtime activation context is too late.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:\"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'\"");
    }
}
