use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));

    #[cfg(target_os = "macos")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        setup_macos(&manifest_dir);
    }

    #[cfg(target_os = "linux")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        setup_linux(&manifest_dir);
    }

    #[cfg(target_os = "windows")]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string()));
        setup_windows(&manifest_dir, &out_dir);
    }
}

fn resolve_ndi_sdk_dir(manifest_dir: &Path) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NDI_SDK_DIR") {
        let path = PathBuf::from(dir);
        if path.exists() {
            return Some(path);
        }
    }

    #[cfg(target_os = "macos")]
    let local = manifest_dir.join("vendor/ndi/macos/sdk");
    #[cfg(target_os = "linux")]
    let local = manifest_dir.join("vendor/ndi/linux/sdk");
    #[cfg(target_os = "windows")]
    let local = manifest_dir.join("vendor/ndi/windows/sdk");

    if local.exists() {
        return Some(local);
    }

    #[cfg(target_os = "macos")]
    let system = Path::new("/Library/NDI SDK for Apple");
    #[cfg(target_os = "linux")]
    let system = Path::new("/usr/share/NDI SDK for Linux");
    #[cfg(target_os = "windows")]
    let system = Path::new(r"C:\Program Files\NDI\NDI 6 SDK");

    if system.exists() {
        return Some(system.to_path_buf());
    }

    None
}

#[cfg(target_os = "macos")]
fn setup_macos(manifest_dir: &Path) {
    // Bundle rpath: allows the packaged binary to find frameworks
    // (Syphon and NDI dylib) inside the .app bundle.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    if let Some(sdk) = resolve_ndi_sdk_dir(manifest_dir) {
        let lib_dir = sdk.join("lib/macOS");
        if lib_dir.exists() {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
        }
        else {
            println!("cargo:warning=NDI library directory not found at {}; app may fail at runtime", lib_dir.display());
        }
    }
    else {
        println!("cargo:warning=NDI SDK not found; app may fail at runtime");
    }

    // Vendored Syphon framework
    let syphon_dir = manifest_dir.join("vendor/syphon");
    let syphon_fw = syphon_dir.join("Syphon.framework");
    if syphon_fw.exists() {
        println!("cargo:rustc-link-arg=-F{}", syphon_dir.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", syphon_dir.display());
    }

    // Link Carbon and CoreFoundation for AppleEvent handling
    println!("cargo:rustc-link-lib=framework=Carbon");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}

#[cfg(target_os = "linux")]
fn setup_linux(manifest_dir: &Path) {
    let sdk = match resolve_ndi_sdk_dir(manifest_dir) {
        Some(sdk) => sdk,
        None => {
            println!("cargo:warning=NDI SDK not found; app may fail at runtime");
            return;
        }
    };

    // The SDK ships one directory per target triple under lib/; grafton-ndi picks
    // the one matching our target to link against. Emitting a rpath for every
    // directory that exists keeps this arch-agnostic and costs nothing at runtime.
    let lib_root = sdk.join("lib");
    match std::fs::read_dir(&lib_root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", entry.path().display());
                }
            }
        }
        Err(_) => {
            println!(
                "cargo:warning=NDI library directory not found at {}; app may fail at runtime",
                lib_root.display()
            );
        }
    }

    // Installed layouts: deb puts resources in usr/lib/<product-name>, and
    // linuxdeploy may relocate them to usr/lib inside an AppDir. Cover both.
    // Cargo does not shell out for link args, so the literal $ORIGIN reaches ld
    // and becomes a DT_RUNPATH entry.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib/Multiviewer");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
}

#[cfg(target_os = "windows")]
fn setup_windows(manifest_dir: &Path, out_dir: &Path) {
    // Embed the application icon as a Windows PE resource so the .exe and
    // shortcuts display it in Explorer.
    let icon_path = manifest_dir.join("assets/AppIcon.ico");
    let rc_path = manifest_dir.join("assets/windows/Multiviewer.rc");
    println!("cargo:rerun-if-changed={}", icon_path.display());
    println!("cargo:rerun-if-changed={}", rc_path.display());
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .expect("failed to compile Windows resources");

    let sdk = match resolve_ndi_sdk_dir(manifest_dir) {
        Some(sdk) => sdk,
        None => {
            println!("cargo:warning=NDI SDK not found; app may fail at runtime");
            return;
        }
    };

    // Link-time: help the linker find the NDI import library.
    let lib_dir = sdk.join("Lib/x64");
    if lib_dir.exists() {
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
    }
    else {
        println!("cargo:warning=NDI import library directory not found at {}", lib_dir.display());
    }

    // Runtime: copy the NDI DLL next to the executable so Windows can find it.
    let dll_name = "Processing.NDI.Lib.x64.dll";
    let dll_source = sdk.join("Bin/x64").join(dll_name);
    if dll_source.exists() {
        let target_dir = derive_target_dir(out_dir);
        let dll_dest = target_dir.join(dll_name);
        match std::fs::copy(&dll_source, &dll_dest) {
            Ok(_) => {
                println!("cargo:rerun-if-changed={}", dll_source.display());
            }
            Err(e) => {
                println!("cargo:warning=Failed to copy NDI DLL from {} to {}: {}", dll_source.display(), dll_dest.display(), e);
            }
        }
    }
    else {
        println!("cargo:warning=NDI runtime DLL not found at {}; app may fail at runtime", dll_source.display());
    }
}

#[cfg(target_os = "windows")]
fn derive_target_dir(out_dir: &Path) -> PathBuf {
    // OUT_DIR is target/<profile>/build/<crate>-<hash>/out
    // (or target/<triple>/<profile>/build/<crate>-<hash>/out when cross-compiling).
    // Three parents up is the profile output directory where the binary is placed.
    out_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| out_dir.to_path_buf())
}
