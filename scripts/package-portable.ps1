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
Copy-Item 'LICENSE-MIT', 'LICENSE-APACHE' $dir
# The debugger: debugpy next to brisk.exe (lib\debugpy), used when the project's Python has none.
& (Join-Path $PSScriptRoot 'fetch-debugpy.ps1')
Copy-Item -Recurse (Join-Path $root 'vendor\debugpy-lib') (Join-Path $dir 'lib')
# Code insight: ty (completion, navigation, type checks) and ruff (lint, format) in lib\, used
# when neither the project interpreter nor PATH has them.
& (Join-Path $PSScriptRoot 'fetch-tools.ps1')
foreach ($t in 'ty', 'ruff') {
    Copy-Item (Join-Path $root "vendor\tools\$t.exe") (Join-Path $dir 'lib')
    Copy-Item (Join-Path $root "vendor\tools\$t.LICENSE.txt") (Join-Path $dir 'lib')
}
# Licenses of the Rust crates and icons compiled into brisk.exe (scripts\third-party-notices.ps1).
Copy-Item 'THIRD-PARTY-NOTICES.txt' $dir
$utf8 = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $dir 'portable'),
    "This file switches Brisk to portable mode: settings, database consoles and saved passwords`r`n" +
    "are kept in the 'data' folder next to brisk.exe instead of %APPDATA%\Brisk.`r`n" +
    "Delete this file to use %APPDATA%\Brisk.`r`n", $utf8)

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
# .NET ZipFile with '/' in entry names: Compress-Archive in Windows PowerShell 5.1 writes '\',
# which unzip and other non-Windows tools take as part of the file name.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$archive = [IO.Compression.ZipFile]::Open($zip, [IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($f in Get-ChildItem -Recurse -File $dir) {
        $entry = $name + '/' + $f.FullName.Substring($dir.Length + 1).Replace('\', '/')
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($archive, $f.FullName, $entry, [IO.Compression.CompressionLevel]::Optimal)
    }
} finally {
    $archive.Dispose()
}
$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
"Packaged $zip ($mb MB)"
