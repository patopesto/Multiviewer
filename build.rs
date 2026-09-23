fn main() {
    #[cfg(target_os = "macos")]
    {
        // Bundle rpath: allows the packaged binary to find frameworks
        // (Syphon and NDI dylib) inside the .app bundle.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

        // Dev builds: point rpath at the locally downloaded SDK.
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let local_sdk = std::path::PathBuf::from(&manifest_dir).join("vendor/ndi/macos/sdk/lib/macOS");
        if local_sdk.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", local_sdk.display());
        }
        else {
            // fallback to system-wide NDI SDK.
            let sdk = std::path::PathBuf::from("/Library/NDI SDK for Apple/lib/macOS");
            if sdk.exists() {
                println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/NDI SDK for Apple/lib/macOS");
            }
        }

        // Vendored Syphon framework
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
        let syphon_dir = std::path::PathBuf::from(&manifest_dir).join("vendor/syphon");
        let syphon_fw = syphon_dir.join("Syphon.framework");
        if syphon_fw.exists() {
            println!("cargo:rustc-link-arg=-F{}", syphon_dir.display());
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", syphon_dir.display());
        }

        // Link Carbon and CoreFoundation for AppleEvent handling
        println!("cargo:rustc-link-lib=framework=Carbon");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }
}
