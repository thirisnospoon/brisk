# Size, startup and memory budget of the release build: one shot, under a minute, for every merge.
# Prints a table and exits with 1 when a limit is exceeded (2 when something cannot be measured).
#
#   powershell -ExecutionPolicy Bypass -File scripts\budget.ps1 [-Build] [-Package] [-Runs 3]
#       [-Exe target\release\brisk.exe] [-Zip dist\x.zip] [-Project examples\demo_project]
#       [-SmokeExe target\smoke\brisk.exe] [-NoMembudget] [-SizeOnly] [-Out results.json]
#
# The work, the limits and what each number means are in `cargo xtask budget`
# (xtask\src\budget.rs, or `cargo xtask budget --help`); this keeps the old command working.
param(
    [switch]$Build,
    [switch]$Package,
    [int]$Runs = 3,
    [string]$Exe,
    [string]$SmokeExe,
    [string]$Zip,
    [string]$Project,
    [switch]$NoMembudget,
    [switch]$SizeOnly,
    [string]$Out
)
$root = Split-Path -Parent $PSScriptRoot
# Paths are taken as given relative to where the script was started, as before.
function Get-Full([string]$p) { if ($p) { [IO.Path]::GetFullPath((Join-Path (Get-Location) $p)) } }
$xtaskArgs = @('xtask', 'budget', '--runs', $Runs)
if ($Build) { $xtaskArgs += '--build' }
if ($Package) { $xtaskArgs += '--package' }
if ($NoMembudget) { $xtaskArgs += '--no-membudget' }
if ($SizeOnly) { $xtaskArgs += '--size-only' }
foreach ($o in @(@('--exe', $Exe), @('--smoke-exe', $SmokeExe), @('--zip', $Zip), @('--project', $Project), @('--out', $Out))) {
    if ($o[1]) { $xtaskArgs += $o[0], (Get-Full $o[1]) }
}
Push-Location $root
try { cargo @xtaskArgs } finally { Pop-Location }
exit $LASTEXITCODE
