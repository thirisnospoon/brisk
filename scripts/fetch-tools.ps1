# Puts ty and ruff (the Windows x64 wheels from PyPI) into vendor\tools, where development
# builds look for them and the portable package copies them from (lib\ty.exe, lib\ruff.exe).
# Brisk prefers the ones in the project interpreter or on PATH; these are the fallback.
# Each tool's license from its wheel is kept next to it as <name>.LICENSE.txt.
# Usage (from the repository root): powershell -ExecutionPolicy Bypass -File scripts\fetch-tools.ps1 [-Ty 0.0.105] [-Ruff 0.17.5]
param([string]$Ty = "0.0.105", [string]$Ruff = "0.17.5")
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$target = Join-Path $root 'vendor\tools'
New-Item -ItemType Directory -Force $target | Out-Null

$python = (Get-Command py -ErrorAction SilentlyContinue).Source
$pyArgs = @('-3')
if (-not $python) { $python = (Get-Command python -ErrorAction Stop).Source; $pyArgs = @() }

foreach ($tool in @(@{ Name = 'ty'; Version = $Ty }, @{ Name = 'ruff'; Version = $Ruff })) {
    $name = $tool.Name
    $version = $tool.Version
    $exe = Join-Path $target "$name.exe"
    $stamp = Join-Path $target "$name.version"
    $license = Join-Path $target "$name.LICENSE.txt"
    if ((Test-Path $exe) -and (Test-Path $license) -and (Test-Path $stamp) -and ((Get-Content $stamp -Raw).Trim() -eq $version)) {
        "$name $version is already in $target"
        continue
    }
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("brisk-$name-" + [guid]::NewGuid())
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        & $python @pyArgs -m pip download "$name==$version" --no-deps --only-binary=:all: --platform win_amd64 -d $tmp
        if ($LASTEXITCODE -ne 0) { throw "pip download $name failed" }
        $wheel = Get-ChildItem $tmp -Filter "$name-*.whl" | Select-Object -First 1
        if (-not $wheel) { throw "no $name wheel was downloaded" }
        $zip = Join-Path $tmp "$name.zip"
        Copy-Item $wheel.FullName $zip
        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -Path $zip -DestinationPath $unpacked -Force
        $found = Get-ChildItem $unpacked -Recurse -Filter "$name.exe" | Select-Object -First 1
        if (-not $found) { throw "$name.exe is not in $($wheel.Name)" }
        Copy-Item $found.FullName $exe -Force
        $lic = Get-ChildItem $unpacked -Recurse -File |
            Where-Object { $_.Directory.FullName -match '\.dist-info' -and $_.Name -match '^(LICEN[CS]E|COPYING)' } |
            Select-Object -First 1
        if (-not $lic) { throw "no license file in $($wheel.Name)" }
        Copy-Item $lic.FullName $license -Force
        Set-Content -Path $stamp -Value $version -Encoding ascii
        "$name $version -> $exe"
    } finally {
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
}
