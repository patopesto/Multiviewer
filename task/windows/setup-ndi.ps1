#Requires -Version 5.1
$ErrorActionPreference = 'Stop'

# Allow overriding with an existing system install.
$sdk_dir = $env:NDI_SDK_DIR
if ($sdk_dir) {
    $sdk_dir = (Resolve-Path $sdk_dir).Path
    Write-Host "Using NDI_SDK_DIR: $sdk_dir"
} elseif (Test-Path "vendor/ndi/windows/sdk/include/Processing.NDI.Lib.h") {
    $sdk_dir = "$((Get-Location).Path)/vendor/ndi/windows/sdk"
    Write-Host "Using existing NDI SDK at $sdk_dir"
}

if (-not $sdk_dir) {
    $accepted_hashes = @()
    foreach ($line in Get-Content $env:NDI_WINDOWS_SHA_FILE) {
        $line = $line.Trim()
        if ($line -eq "" -or $line.StartsWith("#")) { continue }
        $accepted_hashes += $line.ToUpperInvariant()
    }

    if ($accepted_hashes.Count -eq 0) {
        Write-Error "No SHA256 hashes found in $env:NDI_WINDOWS_SHA_FILE"
        exit 1
    }

    $primary_hash = $accepted_hashes[0]
    $cache_dir = "$env:NDI_CACHE_DIR/windows/$env:NDI_VERSION-$($primary_hash.Substring(0,8))"
    $installer = "$cache_dir/ndi-sdk.exe"
    New-Item -ItemType Directory -Force -Path $cache_dir | Out-Null

    if (-not (Test-Path $installer)) {
        Write-Host "Downloading NDI SDK v$env:NDI_VERSION for Windows..."
        Invoke-WebRequest -Uri $env:NDI_WINDOWS_URL -OutFile $installer
    } else {
        Write-Host "Using cached NDI SDK installer at $installer"
    }

    $actual = (Get-FileHash $installer -Algorithm SHA256).Hash
    $hash_ok = $false
    foreach ($h in $accepted_hashes) {
        if ($actual -eq $h) {
            $hash_ok = $true
            break
        }
    }

    if (-not $hash_ok) {
        Write-Error "NDI SDK SHA256 mismatch! Actual: $actual. Accepted: $($accepted_hashes -join ', ')"
        Write-Error "The upstream installer may have been rotated. Update $env:NDI_WINDOWS_SHA_FILE."
        exit 1
    }
    Write-Host "NDI SDK SHA256 verified."

    $sdk_dir = "$((Get-Location).Path)/vendor/ndi/windows/sdk"
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $sdk_dir
    New-Item -ItemType Directory -Force -Path $sdk_dir | Out-Null

    Write-Host "Installing NDI SDK to $sdk_dir..."
    $proc = Start-Process -FilePath $installer `
        -ArgumentList "/VERYSILENT","/SP-","/SUPPRESSMSGBOXES","/NORESTART","/NOCANCEL","/DIR=$sdk_dir","/LOG=$env:TEMP/ndi_install.log" `
        -PassThru
    if (-not $proc.WaitForExit(300000)) {
        $proc | Stop-Process -Force
        if (Test-Path "$sdk_dir/include/Processing.NDI.Lib.h") {
            Write-Host "Installer timed out but SDK files are present"
        } else {
            Write-Error "NDI SDK installation timed out"
            exit 1
        }
    } elseif ($proc.ExitCode -ne 0) {
        Write-Error "NDI installer failed with code $($proc.ExitCode)"
        exit 1
    }

    if (-not (Test-Path "$sdk_dir/Include/Processing.NDI.Lib.h")) {
        Write-Error "NDI SDK installation failed - header file not found"
        exit 1
    }
    Write-Host "NDI SDK installed successfully."
}

# Persist NDI_SDK_DIR for cargo so `cargo run` finds the local SDK.
New-Item -ItemType Directory -Force -Path "$((Get-Location).Path)/.cargo" | Out-Null
$toml = "[env]`nNDI_SDK_DIR = '$((Resolve-Path $sdk_dir).Path)'`n"
$toml | Out-File -FilePath "$((Get-Location).Path)/.cargo/config.toml" -Encoding utf8

Write-Host "NDI SDK v$env:NDI_VERSION ready at $sdk_dir"
