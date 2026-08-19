use std::path::PathBuf;

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

        // Resolve SDK include path based on platform
        let sdk_include = if cfg!(target_os = "macos") {
            manifest_dir.join("../../../../vendor/blackmagic/Mac/include/")
        } else if cfg!(target_os = "linux") {
            manifest_dir.join("../../../../vendor/blackmagic/Linux/include/")
        } else {
            return;
        };

        // Generate bindings from SDK headers
        let bindings = bindgen::Builder::default()
            .header("include/decklink_sdk_enums.h")
            .clang_arg(format!("-I{}", sdk_include.display()))
            .clang_arg("-x")
            .clang_arg("c++")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .allowlist_type("BMD.*")
            .allowlist_var("bmd.*")
            .allowlist_var("_BMD.*")
            .generate()
            .expect("Unable to generate DeckLink SDK bindings");

        bindings
            .write_to_file(out_dir.join("decklink_sdk_bindings.rs"))
            .expect("Unable to write bindings");

        // Compile C++ shim
        let mut build = cc::Build::new();
        build.cpp(true);
        build.include(&sdk_include);
        build.include("shim/");

        if cfg!(target_os = "macos") {
            build.file("shim/decklink_input.cpp");
            build.file("shim/decklink_output.cpp");
            build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=CoreVideo");
        } else if cfg!(target_os = "linux") {
            build.file("shim/decklink_input.cpp");
            build.file("shim/decklink_output.cpp");
            build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
            println!("cargo:rustc-link-lib=dl");
        }

        build.compile("decklink_shim");

        println!("cargo:rerun-if-changed=shim/decklink_input.cpp");
        println!("cargo:rerun-if-changed=shim/decklink_input.h");
        println!("cargo:rerun-if-changed=shim/decklink_output.cpp");
        println!("cargo:rerun-if-changed=shim/decklink_output.h");
        println!("cargo:rerun-if-changed=include/decklink_sdk_enums.h");
    }
}
