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
$exe = Resolve-Path "$out\multiviewer.exe"
$reg = @"
Windows Registry Editor Version 5.00

[HKEY_CURRENT_USER\Software\Classes\.multiviewer]
@="MultiviewerProject"

[HKEY_CURRENT_USER\Software\Classes\MultiviewerProject]
@="Multiviewer Project"

[HKEY_CURRENT_USER\Software\Classes\MultiviewerProject\shell\open\command]
@="\"$($exe -replace '\\','\\\\')\" \"%1\""

[HKEY_CURRENT_USER\Software\Classes\MultiviewerProject\DefaultIcon]
@="$($exe -replace '\\','\\\\'),0"
"@
$reg | Out-File -Encoding utf8 "$out\register-multiviewer.reg"
Write-Host "wrote $out\register-multiviewer.reg"
Write-Host "wrote $out"
