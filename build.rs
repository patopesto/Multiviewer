fn main() {
    #[cfg(target_os = "macos")]
    {
        // Dev builds: point rpath at the local SDK so `cargo run` works.
        // Release packaging rewrites rpath to the bundled copy.
        let sdk = std::path::PathBuf::from("/Library/NDI SDK for Apple/lib/macOS");
        if sdk.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/NDI SDK for Apple/lib/macOS");
        }

        // Vendored Syphon framework
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let syphon_dir = std::path::PathBuf::from(&manifest_dir).join("vendor/syphon");
        let syphon_fw = syphon_dir.join("Syphon.framework");
        if syphon_fw.exists() {
            println!("cargo:rustc-link-arg=-F{}", syphon_dir.display());
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", syphon_dir.display());
        }
    }
}
