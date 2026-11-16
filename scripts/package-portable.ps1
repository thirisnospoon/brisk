# Builds the portable Windows package: dist\Brisk-<version>-windows-x64.zip
# Usage (from the repository root): powershell -ExecutionPolicy Bypass -File scripts\package-portable.ps1 [-NoBuild]
param([switch]$NoBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if (-not $NoBuild) {
    cargo build --release -p brisk-app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$name = "Brisk-$version-windows-x64"
$dist = Join-Path $root 'dist'
$dir = Join-Path $dist $name
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
New-Item -ItemType Directory -Force $dir | Out-Null

Copy-Item 'target\release\brisk.exe' $dir
Copy-Item 'README.md' $dir
$utf8 = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $dir 'portable'),
    "This file switches Brisk to portable mode: settings, database consoles and saved passwords`r`n" +
    "are kept in the 'data' folder next to brisk.exe instead of %APPDATA%\Brisk.`r`n" +
    "Delete this file to use %APPDATA%\Brisk.`r`n", $utf8)

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $dir -DestinationPath $zip
$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
"Packaged $zip ($mb MB)"
