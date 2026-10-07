use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));

    emit_build_info(&manifest_dir);

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

fn emit_build_info(manifest_dir: &Path) {
    println!("cargo:rerun-if-changed=task/Packager.common.toml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let config_version = packager_field(manifest_dir, "version");
    let cargo_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    if config_version != cargo_version {
        panic!("version mismatch: Cargo.toml={cargo_version}, task/Packager.common.toml={config_version}");
    }

    println!("cargo:rustc-env=GIT_COMMIT={}", git_commit(manifest_dir));
    println!("cargo:rustc-env=BUILD_DATE={}", build_date());
    println!("cargo:rustc-env=APP_COPYRIGHT={}", packager_field(manifest_dir, "copyright"));
}

// Read a top-level `key = "value"` string from the shared packager config.
fn packager_field(manifest_dir: &Path, key: &str) -> String {
    let path = manifest_dir.join("task/Packager.common.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix(key) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = rest.trim();
        if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
            return inner.to_string();
        }
    }
    return String::new();
}

fn git_commit(manifest_dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(manifest_dir)
        .output();
    let commit = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    };
    if commit == "unknown" {
        return commit;
    }
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(manifest_dir)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        return format!("{commit}-dirty");
    }
    return commit;
}

fn build_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let seconds_of_day = secs % 86400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    return format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC");
}

// Days since the Unix epoch to a civil (year, month, day); Howard Hinnant's algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = if month <= 2 { y + 1 } else { y };
    return (year, month, day);
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
    // Embed the icon and version metadata as PE resources so Explorer and the
    // installer show the app's name, version and publisher.
    let icon_path = manifest_dir.join("assets/appicon/windows/AppIcon.ico");
    println!("cargo:rerun-if-changed={}", icon_path.display());
    let rc_path = write_windows_rc(manifest_dir, out_dir, &icon_path);
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

// Write the PE resource script (icon + VERSIONINFO) from the shared packager
// config and Cargo metadata, so the .exe metadata never drifts.
#[cfg(target_os = "windows")]
fn write_windows_rc(manifest_dir: &Path, out_dir: &Path, icon_path: &Path) -> PathBuf {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let crate_name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    let product = packager_field(manifest_dir, "product-name");
    let description = packager_field(manifest_dir, "description");
    let copyright = packager_field(manifest_dir, "copyright");
    let publisher = packager_field(manifest_dir, "publisher");
    let icon = icon_path.display().to_string().replace('\\', "/");
    let (major, minor, patch) = version_triple(&version);

    let rc = format!(
        r#"#pragma code_page(65001)
#include <winver.h>

1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION     {major},{minor},{patch},0
PRODUCTVERSION  {major},{minor},{patch},0
FILEFLAGSMASK   0x3fL
FILEFLAGS       0x0L
FILEOS          VOS_NT_WINDOWS32
FILETYPE        VFT_APP
FILESUBTYPE     VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName",      "{publisher}"
            VALUE "FileDescription",  "{description}"
            VALUE "FileVersion",      "{version}"
            VALUE "InternalName",     "{crate_name}"
            VALUE "LegalCopyright",   "{copyright}"
            VALUE "OriginalFilename", "{crate_name}.exe"
            VALUE "ProductName",      "{product}"
            VALUE "ProductVersion",   "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let rc_path = out_dir.join("Multiviewer.rc");
    std::fs::write(&rc_path, rc).expect("failed to write Windows resource file");
    return rc_path;
}

#[cfg(target_os = "windows")]
fn version_triple(version: &str) -> (u16, u16, u16) {
    let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    return (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
}
