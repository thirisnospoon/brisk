# Builds the portable Windows package: dist\Brisk-<version>-windows-x64.zip
# Usage (from the repository root): powershell -ExecutionPolicy Bypass -File scripts\package-portable.ps1 [-NoBuild]
# The work is done by `cargo xtask package-portable` (xtask\src\package.rs); this keeps the old
# command working.
param([switch]$NoBuild)
$root = Split-Path -Parent $PSScriptRoot
$xtaskArgs = @('xtask', 'package-portable')
if ($NoBuild) { $xtaskArgs += '--no-build' }
Push-Location $root
try { cargo @xtaskArgs } finally { Pop-Location }
exit $LASTEXITCODE
