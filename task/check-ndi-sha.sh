#!/usr/bin/env bash
# Verify the pinned SHA256 of every platform's NDI SDK installer.
#
# Runs from the repository root. Expected environment (set by task/Taskfile.ndi.yml):
#   NDI_VERSION                    SDK version, from vendor/ndi/NDI_SDK_VERSION
#   NDI_CACHE_DIR                  download cache root
#   NDI_<PLATFORM>_SDK_URL         installer URL, per platform
#   NDI_<PLATFORM>_SHA_FILE        allowlist of accepted SHA256 hashes
#   NDI_<PLATFORM>_INSTALLER       installer filename inside the cache dir
#
# <PLATFORM> is the OS name in uppercase (MACOS, WINDOWS, LINUX). Reads only:
# nothing under vendor/ndi is modified except the gitignored download cache.
set -euo pipefail

mismatches=0

for platform in macos windows linux; do
    platform_var=$(echo "$platform" | tr '[:lower:]' '[:upper:]')
    sdk_url_var="NDI_${platform_var}_SDK_URL"
    sha_file_var="NDI_${platform_var}_SHA_FILE"
    installer_var="NDI_${platform_var}_INSTALLER"
    sdk_url=${!sdk_url_var}
    sha_file=${!sha_file_var}
    installer=${!installer_var}

    # The hash in a cache dir name comes from the sha file, which may just have
    # been updated, so key on the version prefix and ignore the rest.
    cache_dir=""
    for candidate in "$NDI_CACHE_DIR/$platform/$NDI_VERSION"-*/; do
        [ -d "$candidate" ] || continue
        if [ -z "$cache_dir" ] || [ "$candidate" -nt "$cache_dir" ]; then
            cache_dir="$candidate"
        fi
    done

    if [ -n "$cache_dir" ]; then
        echo "$platform: using cached $cache_dir$installer"
    else
        cache_dir="$NDI_CACHE_DIR/$platform/$NDI_VERSION-unchecked/"
        mkdir -p "$cache_dir"
        echo "$platform: downloading $sdk_url"
        # --fail so a 404 error page is never hashed and reported as a pin.
        curl -fL -o "$cache_dir$installer" "$sdk_url"
    fi

    actual=$(shasum -a 256 "$cache_dir$installer" | cut -d' ' -f1 | tr '[:lower:]' '[:upper:]')

    # Membership, not iteration: the sha file is a comment-prefixed allowlist and
    # the pins are stored in mixed case.
    if grep -qiF "$actual" "$sha_file"; then
        echo "$platform: OK"
        echo "  actual: $actual"
        echo "  pinned: $sha_file"
        # Hand the download to task ndi:setup:*, which looks the installer up by
        # the primary hash; leave a mismatch alone, its version prefix makes the
        # next run reuse it after the sha file is updated.
        if [ "$(basename "$cache_dir")" = "$NDI_VERSION-unchecked" ]; then
            primary=$(grep -v -e '^#' -e '^[[:space:]]*$' "$sha_file" | head -1 | tr -d '[:space:]' | tr '[:lower:]' '[:upper:]')
            mv "$cache_dir" "$NDI_CACHE_DIR/$platform/$NDI_VERSION-$primary"
        fi
        continue
    fi

    echo "$platform: MISMATCH" >&2
    echo "  actual: $actual" >&2
    echo "  pinned: $sha_file" >&2
    echo "  replace the primary hash in that file with:" >&2
    echo "    $actual" >&2
    mismatches=$((mismatches + 1))
done

exit $((mismatches > 0))