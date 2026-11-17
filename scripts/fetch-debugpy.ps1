# Puts debugpy (the pure-Python wheel from PyPI) into vendor\debugpy-lib, where development
# builds look for it and the portable package copies it from. Needs a Python with pip.
# Usage (from the repository root): powershell -ExecutionPolicy Bypass -File scripts\fetch-debugpy.ps1 [-Version 1.8.22]
param([string]$Version = "1.8.22")
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$target = Join-Path $root 'vendor\debugpy-lib'
if (Test-Path (Join-Path $target 'debugpy\__init__.py')) { "debugpy is already in $target"; return }

$python = (Get-Command py -ErrorAction SilentlyContinue).Source
$pyArgs = @('-3')
if (-not $python) { $python = (Get-Command python -ErrorAction Stop).Source; $pyArgs = @() }

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("brisk-debugpy-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    & $python @pyArgs -m pip download "debugpy==$Version" --no-deps --only-binary=:all: --platform any -d $tmp
    if ($LASTEXITCODE -ne 0) { throw "pip download failed" }
    $wheel = Get-ChildItem $tmp -Filter 'debugpy-*-none-any.whl' | Select-Object -First 1
    if (-not $wheel) { throw "no pure-Python debugpy wheel was downloaded" }
    $zip = Join-Path $tmp 'debugpy.zip'
    Copy-Item $wheel.FullName $zip
    New-Item -ItemType Directory -Force $target | Out-Null
    Expand-Archive -Path $zip -DestinationPath $target -Force
    "debugpy $Version -> $target"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
