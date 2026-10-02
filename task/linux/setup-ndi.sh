#!/usr/bin/env bash
# Download, verify, and extract the Linux NDI SDK.
#
# Runs from the repository root. Expected environment (set by task/Taskfile.ndi.yml):
#   NDI_VERSION       SDK version, from vendor/ndi/NDI_SDK_VERSION
#   NDI_LINUX_URL     installer URL
#   NDI_LINUX_SHA_FILE  allowlist of accepted SHA256 hashes
#   NDI_CACHE_DIR     download cache root
#
# Overridable with NDI_SDK_DIR to point at an existing install.
set -euo pipefail

sdk_dir="${NDI_SDK_DIR:-$PWD/vendor/ndi/linux/sdk}"
case "$sdk_dir" in
    /*) ;;
    *) sdk_dir="$PWD/$sdk_dir" ;;
esac

if [ ! -f "$sdk_dir/include/Processing.NDI.Lib.h" ]; then
    # Replacing an existing install only makes sense inside the repo; an
    # NDI_SDK_DIR pointing elsewhere would otherwise be deleted.
    case "$sdk_dir" in
        "$PWD"/*) ;;
        *)
            echo "$sdk_dir exists without a usable header; refusing to replace it" >&2
            exit 1
            ;;
    esac

    accepted_hashes=()
    while IFS= read -r line || [ -n "$line" ]; do
        line=$(echo "$line" | tr -d '[:space:]')
        [ -z "$line" ] && continue
        case "$line" in \#*) continue ;; esac
        accepted_hashes+=("$(echo "$line" | tr '[:lower:]' '[:upper:]')")
    done < "$NDI_LINUX_SHA_FILE"

    if [ ${#accepted_hashes[@]} -eq 0 ]; then
        echo "No SHA256 hashes found in $NDI_LINUX_SHA_FILE" >&2
        exit 1
    fi

    primary_hash="${accepted_hashes[0]}"
    cache_dir="$NDI_CACHE_DIR/linux/$NDI_VERSION-$primary_hash"
    installer_tar="$cache_dir/ndi-sdk.tar.gz"
    mkdir -p "$cache_dir"

    if [ ! -f "$installer_tar" ]; then
        echo "Downloading NDI SDK v$NDI_VERSION for Linux..."
        curl -L -o "$installer_tar" "$NDI_LINUX_URL"
    else
        echo "Using cached NDI SDK installer at $installer_tar"
    fi

    actual=$(sha256sum "$installer_tar" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')
    hash_ok=false
    for h in "${accepted_hashes[@]}"; do
        if [ "$actual" = "$h" ]; then
            hash_ok=true
            break
        fi
    done

    if [ "$hash_ok" != true ]; then
        echo "NDI SDK SHA256 mismatch!" >&2
        echo "Expected: ${accepted_hashes[*]}" >&2
        echo "Actual:   $actual" >&2
        echo "The upstream installer may have been rotated. Update $NDI_LINUX_SHA_FILE." >&2
        exit 1
    fi
    echo "NDI SDK SHA256 verified."

    # The payload is a gzip'd tarball appended to the installer script after a
    # marker line; there is nothing to execute, only a boundary to slice at.
    work="$cache_dir/_extract"
    rm -rf "$work"
    mkdir -p "$work"
    tar -xzf "$installer_tar" -C "$work"

    installer_sh=$(find "$work" -name 'Install_NDI_SDK_*.sh' -type f -print -quit)
    if [ -z "$installer_sh" ]; then
        echo "Could not find the NDI installer script in $work" >&2
        exit 1
    fi

    archive_line=$(awk '/^__NDI_ARCHIVE_BEGIN__/ { print NR+1; exit 0 }' "$installer_sh")
    if [ -z "$archive_line" ]; then
        echo "Could not find __NDI_ARCHIVE_BEGIN__ in $installer_sh" >&2
        exit 1
    fi
    tail -n +"$archive_line" "$installer_sh" | tar -xzf - -C "$work"

    payload=$(find "$work" -maxdepth 1 -mindepth 1 -type d -name '*NDI*' -print -quit)
    if [ -z "$payload" ]; then
        echo "Could not find the extracted NDI SDK directory in $work" >&2
        ls -la "$work" >&2
        exit 1
    fi

    rm -rf "$sdk_dir"
    mkdir -p "$sdk_dir"
    cp -R "$payload"/. "$sdk_dir"/
    rm -rf "$work"

    # The SDK ships only versioned libraries; the linker needs the unversioned
    # name (and dlopen/DT_NEEDED want the soname) in each arch directory.
    while IFS= read -r lib; do
        dir=$(dirname "$lib")
        name=$(basename "$lib")
        base=${name%%.so.*}
        version=${name#"$base".so.}
        major=${version%%.*}
        # ln -sfn of a name onto itself would replace the real file.
        [ "$name" = "$base.so.$major" ] || ln -sfn "$name" "$dir/$base.so.$major"
        ln -sfn "$name" "$dir/$base.so"
    done < <(find "$sdk_dir/lib" -mindepth 2 -maxdepth 2 -type f -name '*.so.*')

    # grafton-ndi only looks for lib/aarch64-linux-gnu, but the SDK ships the
    # arm64 build as a Raspberry Pi triple (note: gnueabi, not gnueabihf), so
    # the name it expects has to be provided as an alias.
    if [ -d "$sdk_dir/lib/aarch64-rpi4-linux-gnueabi" ] && [ ! -e "$sdk_dir/lib/aarch64-linux-gnu" ]; then
        ln -s aarch64-rpi4-linux-gnueabi "$sdk_dir/lib/aarch64-linux-gnu"
    fi

    # One arch-independent path for the packager config, which cannot be templated.
    case "$(uname -m)" in
        x86_64)  arch_dir=x86_64-linux-gnu ;;
        aarch64 | arm64) arch_dir=aarch64-linux-gnu ;;
        armv7l)  arch_dir=arm-rpi4-linux-gnueabihf ;;
        armv6l)  arch_dir=arm-rpi1-linux-gnueabihf ;;
        i?86)    arch_dir=i686-linux-gnu ;;
        *)       arch_dir="" ;;
    esac

    if [ -z "$arch_dir" ] || [ ! -f "$sdk_dir/lib/$arch_dir/libndi.so" ]; then
        echo "No NDI library for $(uname -m) under $sdk_dir/lib/" >&2
        ls -la "$sdk_dir/lib" >&2
        exit 1
    fi
    ln -sfn "lib/$arch_dir/libndi.so" "$sdk_dir/libndi.so"
fi

# Persist NDI_SDK_DIR for cargo so `cargo run` finds the local SDK.
mkdir -p "$PWD/.cargo"
printf '[env]\nNDI_SDK_DIR = "%s"\n' "$(cd "$sdk_dir" && pwd)" > "$PWD/.cargo/config.toml"

echo "NDI SDK v$NDI_VERSION ready at $sdk_dir"
