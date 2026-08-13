fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let mut build = cc::Build::new();
        build.cpp(true);
        build.file("src/sources/decklink/decklink_shim.cpp");

        if cfg!(target_os = "macos") {
            build.include("vendor/blackmagic/Mac/include/");
            build.file("vendor/blackmagic/Mac/include/DeckLinkAPIDispatch.cpp");
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreVideo");
        } else if cfg!(target_os = "linux") {
            build.include("vendor/blackmagic/Linux/include/");
            build.file("vendor/blackmagic/Linux/include/DeckLinkAPIDispatch.cpp");
            println!("cargo:rustc-link-lib=dl");
        }

        build.compile("decklink_shim");

        println!("cargo:rerun-if-changed=src/sources/decklink/decklink_shim.cpp");
        println!("cargo:rerun-if-changed=src/sources/decklink/decklink_shim.h");
    }

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
