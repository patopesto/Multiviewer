#!/bin/sh
# Package multiviewer for the current OS (macOS .app / Linux tarball).
# Bundles the vendored NDI runtime if present (see vendor/ndi/README.md).
set -eu
cd "$(dirname "$0")/.."

APP=Multiviewer
cargo build --release

case "$(uname -s)" in
Darwin)
    BUNDLE="dist/$APP.app"
    rm -rf "$BUNDLE"
    mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Frameworks"
    cp "target/release/$APP" "$BUNDLE/Contents/MacOS/"
    cat > "$BUNDLE/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Multiviewer</string>
  <key>CFBundleExecutable</key><string>Multiviewer</string>
  <key>CFBundleIdentifier</key><string>com.multiviewer.app</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Multiviewer Project</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>CFBundleTypeExtensions</key>
      <array><string>multiviewer</string></array>
      <key>LSItemContentTypes</key>
      <array><string>com.multiviewer.project</string></array>
      <key>LSHandlerRank</key><string>Owner</string>
    </dict>
  </array>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <key>UTTypeIdentifier</key><string>com.multiviewer.project</string>
      <key>UTTypeDescription</key><string>Multiviewer Project</string>
      <key>UTTypeConformsTo</key>
      <array><string>public.data</string></array>
      <key>UTTypeTagSpecification</key>
      <dict>
        <key>public.filename-extension</key>
        <array><string>multiviewer</string></array>
      </dict>
    </dict>
  </array>
</dict></plist>
EOF
    SDK_LIB="/Library/NDI SDK for Apple/lib/macOS/libndi.dylib"
    VEND_DIR="vendor/ndi/macos"
    mkdir -p "$VEND_DIR"
    if [ -f "$SDK_LIB" ]; then
        cp "$SDK_LIB" "$VEND_DIR/"
        echo "copied NDI runtime from SDK"
    fi
    if ls "$VEND_DIR"/*.dylib >/dev/null 2>&1; then
        cp "$VEND_DIR"/*.dylib "$BUNDLE/Contents/Frameworks/"
        install_name_tool -add_rpath @executable_path/../Frameworks "$BUNDLE/Contents/MacOS/$APP" 2>/dev/null || true
        echo "bundled NDI runtime"
    else
        echo "warning: no NDI runtime found, NDI will be unavailable" >&2
    fi
    # Bundle vendored Syphon framework
    if [ -d "vendor/syphon/Syphon.framework" ]; then
        cp -R "vendor/syphon/Syphon.framework" "$BUNDLE/Contents/Frameworks/"
        echo "bundled Syphon framework"
    else
        echo "warning: vendor/syphon/Syphon.framework not found, Syphon will be unavailable" >&2
    fi
    codesign --force --deep -s - "$BUNDLE" 2>/dev/null || true
    echo "wrote $BUNDLE"
    ;;
Linux)
    OUT="dist/$APP-linux"
    rm -rf "$OUT"
    mkdir -p "$OUT"
    cp "target/release/$APP" "$OUT/"
    VEND_DIR="vendor/ndi/linux"
    if ls "$VEND_DIR"/*.so* >/dev/null 2>&1; then
        cp -a "$VEND_DIR"/*.so* "$OUT/"
        command -v patchelf >/dev/null && patchelf --set-rpath '$ORIGIN' "$OUT/$APP" || true
        echo "bundled NDI runtime"
    else
        echo "warning: vendor/ndi/linux is empty, NDI will be unavailable" >&2
    fi
    cat > "$OUT/multiviewer.desktop" <<'EOF'
[Desktop Entry]
Name=Multiviewer
Comment=Multi-protocol video multiviewer
Exec=multiviewer %f
Type=Application
Terminal=false
Icon=multiviewer
Categories=AudioVideo;Video;
MimeType=application/x-multiviewer-project;
EOF
    cat > "$OUT/multiviewer-mime.xml" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-multiviewer-project">
    <comment>Multiviewer Project</comment>
    <glob pattern="*.multiviewer"/>
  </mime-type>
</mime-info>
EOF
    tar -czf "dist/$APP-linux.tar.gz" -C dist "$APP-linux"
    echo "wrote dist/$APP-linux.tar.gz"
    echo "To register the .multiviewer file association, run:"
    echo "  xdg-mime install $OUT/multiviewer-mime.xml"
    echo "  xdg-desktop-menu install $OUT/multiviewer.desktop"
    ;;
*)
    echo "unsupported OS: $(uname -s) (use scripts/package.ps1 on Windows)" >&2
    exit 1
    ;;
esac
