#!/usr/bin/env bash
# Download, verify, and extract the macOS NDI SDK.
#
# Runs from the repository root. Expected environment (set by task/Taskfile.ndi.yml):
#   NDI_VERSION       SDK version, from vendor/ndi/NDI_SDK_VERSION
#   NDI_SDK_URL       installer URL
#   NDI_SHA_FILE      allowlist of accepted SHA256 hashes
#   NDI_CACHE_DIR     download cache root
#
# Overridable with NDI_SDK_DIR to point at an existing install.
set -euo pipefail

sdk_dir="${NDI_SDK_DIR:-$PWD/vendor/ndi/macos/sdk}"
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
    done < "$NDI_SHA_FILE"

    if [ ${#accepted_hashes[@]} -eq 0 ]; then
        echo "No SHA256 hashes found in $NDI_SHA_FILE" >&2
        exit 1
    fi

    primary_hash="${accepted_hashes[0]}"
    cache_dir="$NDI_CACHE_DIR/macos/$NDI_VERSION-$primary_hash"
    pkg="$cache_dir/ndi-sdk.pkg"
    mkdir -p "$cache_dir"

    if [ ! -f "$pkg" ]; then
        echo "Downloading NDI SDK v$NDI_VERSION for macOS..."
        curl -L -o "$pkg" "$NDI_SDK_URL"
    else
        echo "Using cached NDI SDK pkg at $pkg"
    fi

    actual=$(shasum -a 256 "$pkg" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')
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
        echo "The upstream installer may have been rotated. Update $NDI_SHA_FILE." >&2
        exit 1
    fi
    echo "NDI SDK SHA256 verified."

    rm -rf "$sdk_dir"
    mkdir -p "$sdk_dir"

    echo "Extracting NDI SDK..."
    pkgutil --expand-full "$pkg" "$sdk_dir/_pkg"

    payload="$sdk_dir/_pkg/NDI_SDK_Component.pkg/Payload"
    if [ ! -d "$payload/NDI SDK for Apple" ]; then
        echo "Could not find NDI SDK payload at $payload/NDI SDK for Apple" >&2
        exit 1
    fi

    mv "$payload/NDI SDK for Apple"/* "$sdk_dir/"
    rm -rf "$sdk_dir/_pkg"
fi

# Persist NDI_SDK_DIR for cargo so `cargo run` finds the local SDK.
mkdir -p "$PWD/.cargo"
printf '[env]\nNDI_SDK_DIR = "%s"\n' "$(cd "$sdk_dir" && pwd)" > "$PWD/.cargo/config.toml"

echo "NDI SDK v$NDI_VERSION ready at $sdk_dir"
