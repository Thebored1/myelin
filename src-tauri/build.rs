fn main() {
    // On Linux, tectonic statically links harfbuzz and leaks its symbols into
    // the global dynamic symbol table, where they hijack the system
    // freetype/pango harfbuzz calls (incompatible ABI) and segfault the GUI on
    // the first text layout. Hide all static-archive symbols from the binary's
    // dynamic symbol table so the system harfbuzz is used for rendering.
    // Applied at final link only, so dependency crates stay cached.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-Wl,--exclude-libs,ALL");
    }
    tauri_build::build()
}
