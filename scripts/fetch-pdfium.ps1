# Downloads the pinned pdfium build into vendor\pdfium\, where
# crates\thermal-ui\build.rs embeds it into the executable. Run once per build
# machine; end users never need it.
#
#   powershell -ExecutionPolicy Bypass -File scripts\fetch-pdfium.ps1
#
# Source: https://github.com/bblanchon/pdfium-binaries (BSD-3).
# The release must match the `pdfium_XXXX` feature of pdfium-render in
# crates\thermal-ui\Cargo.toml; a different build can be missing functions the
# bindings need.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$release = 'chromium/7543'

$scriptDirectory = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$root = (Resolve-Path (Join-Path $scriptDirectory '..')).Path

$isArm = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq `
    [System.Runtime.InteropServices.Architecture]::Arm64
if ($isArm) {
    $asset = 'pdfium-win-arm64.tgz'; $dir = 'windows-aarch64'
} else {
    $asset = 'pdfium-win-x64.tgz'; $dir = 'windows-x86_64'
}

$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$($release -replace '/', '%2F')/$asset"
$work = Join-Path ([System.IO.Path]::GetTempPath()) ("pdfium-" + [Guid]::NewGuid().ToString('n'))
New-Item -ItemType Directory -Force -Path $work | Out-Null

try {
    Write-Host "Downloading $asset ($release)"
    $archive = Join-Path $work $asset
    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($curl) {
        & $curl.Source -fsSL -o $archive $url
        if ($LASTEXITCODE -ne 0) { throw "curl failed with exit code $LASTEXITCODE" }
    } else {
        Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    }

    & tar -xzf $archive -C $work
    if ($LASTEXITCODE -ne 0) { throw "tar failed with exit code $LASTEXITCODE" }

    $library = Get-ChildItem -Path $work -Recurse -File -Filter 'pdfium.dll' | Select-Object -First 1
    if (-not $library) { throw "no pdfium.dll inside $asset" }

    $dest = Join-Path $root "vendor\pdfium\$dir"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item $library.FullName (Join-Path $dest 'pdfium.dll') -Force
    Copy-Item (Join-Path $work 'LICENSE') (Join-Path $dest 'LICENSE') -Force
    Write-Host "pdfium $release -> $dest\pdfium.dll"
    Write-Host 'Build with: cargo build --release'
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
