#[cfg(feature = "extended-siginfo-raw")]
fn main() {
    // Only supported for non-Windows targets
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        println!("cargo:rerun-if-changed=src/low_level/extract.c");
        cc::Build::new()
            .file("src/low_level/extract.c")
            .compile("extract");
    }
}

#[cfg(not(feature = "extended-siginfo-raw"))]
fn main() {}
