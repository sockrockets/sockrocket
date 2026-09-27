# Pack the Windows GUI into a ZIP with branded exe + icon assets.
#
# Usage (PowerShell):
#   .\tools\windows_package.ps1 -Binary path\to\sockrocket.exe -Version 0.1.0 -Out path\to\sockrocket-windows-x86_64.zip
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Out
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $Binary)) {
    throw "Binary not found: $Binary"
}

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Assets = Join-Path $Root "crates\sockrocket-gui\assets"
$Ico = Join-Path $Assets "icon.ico"
$Png512 = Join-Path $Assets "icon-512.png"

$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ("sockrocket-win-" + [guid]::NewGuid().ToString("N"))
$Pkg = Join-Path $Stage "Sockrocket"
New-Item -ItemType Directory -Path $Pkg -Force | Out-Null

try {
    Copy-Item $Binary (Join-Path $Pkg "Sockrocket.exe")

    if (Test-Path $Ico) {
        Copy-Item $Ico (Join-Path $Pkg "Sockrocket.ico")
    }
    if (Test-Path $Png512) {
        Copy-Item $Png512 (Join-Path $Pkg "Sockrocket.png")
    }

    @"
Sockrocket $Version (Windows x86_64)
====================================

1. Extract this ZIP anywhere.
2. Double-click Sockrocket.exe (icon is embedded in the executable).
3. Optional: right-click Sockrocket.exe → Create shortcut → pin to taskbar/Start.
   You can also set a custom shortcut icon to Sockrocket.ico.

Default listeners:
  SOCKS5  127.0.0.1:1080
  HTTP    127.0.0.1:1087

Docs: https://github.com/sockrockets/sockrocket
"@ | Set-Content -Path (Join-Path $Pkg "README.txt") -Encoding UTF8

    $OutDir = Split-Path -Parent $Out
    if ($OutDir -and -not (Test-Path $OutDir)) {
        New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
    }
    if (Test-Path $Out) { Remove-Item $Out -Force }

    Compress-Archive -Path $Pkg -DestinationPath $Out -CompressionLevel Optimal
    Write-Host "Built: $Out"
}
finally {
    if (Test-Path $Stage) {
        Remove-Item $Stage -Recurse -Force
    }
}
