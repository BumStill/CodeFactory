fn main() {
    // The formal unattended smoke re-executes this exact binary as workers.
    // On Windows MSVC those workers start on the PE main thread, whose default
    // 1 MiB stack overflows in the deeply nested recovery future before fault
    // injection (0xc00000fd). RUST_MIN_STACK only affects Rust-spawned threads,
    // so set the executable stack reserve at link time instead.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }
    tauri_build::build()
}
