fn main() {
    #[cfg(target_os = "macos")]
    {
        // Dev builds: point rpath at the local SDK so `cargo run` works.
        // Release packaging rewrites rpath to the bundled copy.
        let sdk = std::path::PathBuf::from("/Library/NDI SDK for Apple/lib/macOS");
        if sdk.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/NDI SDK for Apple/lib/macOS");
        }
    }
}
