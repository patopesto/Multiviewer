use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let platform_impl = match target_os.as_str() {
        "macos" => "shim/platform_macos.cpp",
        "linux" | "windows" => "shim/platform_cpu.cpp",
        _ => "",
    };

    for file in [
        "shim/platform.h",
        "shim/platform.cpp",
        "shim/platform_cpu.cpp",
        "shim/platform_macos.cpp",
        "shim/decklink_discovery.cpp",
        "shim/decklink_discovery.h",
        "shim/decklink_input.cpp",
        "shim/decklink_input.h",
        "shim/decklink_output.cpp",
        "shim/decklink_output.h",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }

    let mut build = cc::Build::new();
    build.cpp(true);
    build.include("shim/");

    let ok = match target_os.as_str() {
        "macos" => setup_macos(&mut build, &manifest_dir),
        "linux" => setup_linux(&mut build, &manifest_dir),
        "windows" => setup_windows(&mut build, &manifest_dir),
        _ => false,
    };
    if !ok {
        return;
    }

    build.file("shim/platform.cpp");
    build.file("shim/decklink_discovery.cpp");
    build.file("shim/decklink_input.cpp");
    build.file("shim/decklink_output.cpp");
    if !platform_impl.is_empty() {
        build.file(platform_impl);
    }

    build.compile("decklink_shim");
}

fn setup_macos(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    let sdk_include = manifest_dir.join("../../../../vendor/blackmagic/Mac/include/");
    build.include(&sdk_include);
    build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=CoreVideo");
    return true;
}

fn setup_linux(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    let sdk_include = manifest_dir.join("../../../../vendor/blackmagic/Linux/include/");
    build.include(&sdk_include);
    build.file(sdk_include.join("DeckLinkAPIDispatch.cpp"));
    println!("cargo:rustc-link-lib=dl");
    return true;
}

fn setup_windows(build: &mut cc::Build, manifest_dir: &std::path::Path) -> bool {
    if !std::env::var("HOST").unwrap_or_default().contains("-windows") {
        println!(
            "cargo:warning=DeckLink Windows shim skipped: compiling it requires a Windows host"
        );
        return false;
    }

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
