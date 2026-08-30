//! Embeds `app.manifest` into the executable.
//!
//! Done through the MSVC linker's own manifest support rather than a `.rc` file, so the build
//! needs no resource compiler. On any other toolchain this is a no-op and the dialog code is
//! compiled out anyway.
fn main() {
    println!("cargo:rerun-if-changed=app.manifest");

    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !(windows && msvc) {
        return;
    }

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
