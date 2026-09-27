use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());

    println!("cargo:rerun-if-changed=shim/platform.h");
    println!("cargo:rerun-if-changed=shim/platform.cpp");
    println!("cargo:rerun-if-changed=shim/decklink_input.cpp");
    println!("cargo:rerun-if-changed=shim/decklink_input.h");
    println!("cargo:rerun-if-changed=shim/decklink_output.cpp");
    println!("cargo:rerun-if-changed=shim/decklink_output.h");

    let mut build = cc::Build::new();
    build.cpp(true);
    build.include("shim/");

    #[cfg(target_os = "macos")]
    if !setup_macos(&mut build, &manifest_dir) {
        return;
    }

    #[cfg(target_os = "linux")]
    if !setup_linux(&mut build, &manifest_dir) {
        return;
    }

    #[cfg(target_os = "windows")]
    if !setup_windows(&mut build, &manifest_dir) {
        return;
    }

    // Common shim files — compiled on all platforms
    build.file("shim/platform.cpp");
    build.file("shim/decklink_input.cpp");
    build.file("shim/decklink_output.cpp");

    build.compile("decklink_shim");
}

#[cfg(target_os = "macos")]
fn setup_macos(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    let sdk_include = manifest_dir.join("../../../../vendor/blackmagic/Mac/include/");
    build.include(&sdk_include);
    build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    return true;
}

#[cfg(target_os = "linux")]
fn setup_linux(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    let sdk_include = manifest_dir.join("../../../../vendor/blackmagic/Linux/include/");
    build.include(&sdk_include);
    build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
    println!("cargo:rustc-link-lib=dl");
    return true;
}

#[cfg(target_os = "windows")]
fn setup_windows(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    let sdk_include = manifest_dir.join("../../../../vendor/blackmagic/Win/include/");
    let generated = sdk_include.join("generated");
    let generated_h = generated.join("DeckLinkAPI.h");
    let generated_iid = generated.join("DeckLinkAPI_i.c");

    if !generated_h.exists() || !generated_iid.exists() {
        println!(
            "cargo:warning=DeckLink Windows generated headers not found at {}. Run 'task windows:generate:decklink-headers' to produce them.",
            generated.display()
        );
        return false;
    }

    build.include(&sdk_include);
    build.include(&generated);
    build.file(&generated_iid);
    println!("cargo:rustc-link-lib=ole32");
    println!("cargo:rustc-link-lib=oleaut32");
    return true;
}
