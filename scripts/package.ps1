# Package multiviewer for Windows. Bundles vendored NDI runtime if present.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release

$out = "dist\multiviewer-win"
Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "target\release\multiviewer.exe" $out

$ndi = "vendor\ndi\windows\*.dll"
if (Test-Path $ndi) {
    Copy-Item $ndi $out
    Write-Host "bundled NDI runtime"
} else {
    Write-Warning "vendor/ndi/windows is empty, NDI will be unavailable"
}
Write-Host "wrote $out"
