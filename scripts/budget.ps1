# Size, startup and memory budget of the release build: one shot, under a minute, for every merge.
# Prints a table and exits with 1 when a limit is exceeded (2 when something cannot be measured).
#
#   powershell -ExecutionPolicy Bypass -File scripts\budget.ps1 [-Build] [-Package] [-Runs 3]
#       [-Exe target\release\brisk.exe] [-Zip dist\x.zip] [-Project examples\demo_project]
#       [-NoMembudget] [-SizeOnly] [-Out results.json]
#
# -SizeOnly checks exe_mb and zip_mb and launches nothing. CI uses it: start time and memory
# on shared runners vary too much to fail a build on.
#
# The limits are $Limits right below. Reported, not checked: start_cold_ms (the first launch),
# the startup profile (named moments since process start), private bytes, and the peak working
# sets of ty and ruff during MEMBUDGET. Every launch gets a fresh APPDATA in %TEMP%, so the
# machine's settings and history do not count; MEMBUDGET runs on a temporary copy of the project
# (it writes files there).
param(
    [switch]$Build,
    [switch]$Package,
    [int]$Runs = 3,
    [string]$Exe,
    [string]$Zip,
    [string]$Project,
    [switch]$NoMembudget,
    [switch]$SizeOnly,
    [string]$Out
)
# MB are working sets.
$Limits = [ordered]@{
    exe_mb       = 26    # release brisk.exe
    zip_mb       = 39    # the portable zip (-Zip, else the newest dist\*.zip; -Package rebuilds it)
    start_ms     = 400   # process start -> first window frame drawn, median of -Runs launches
    idle_mb      = 110   # brisk.exe 5 s after the first frame, project open, nothing else
    membudget_mb = 200   # BRISK_SMOKE_MEMBUDGET at its end: 200k-line run, 50 tabs, Find in Files
}
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Exe) { $Exe = Join-Path $root 'target\release\brisk.exe' }
if (-not $Project) { $Project = Join-Path $root 'examples\demo_project' }
$Exe = [IO.Path]::GetFullPath($Exe)
$Project = [IO.Path]::GetFullPath($Project)

if ($Build) {
    Push-Location $root
    try {
        cargo build --release -p brisk-app
        if ($LASTEXITCODE -ne 0) { Write-Host "cargo build failed"; exit 2 }
    } finally { Pop-Location }
}
if (-not (Test-Path $Exe)) { Write-Host "No $Exe (pass -Build, or -Exe)"; exit 2 }
if ($Package) {
    & (Join-Path $PSScriptRoot 'package-portable.ps1') -NoBuild
}

function Mb($bytes) { [math]::Round($bytes / 1MB, 1) }

$r = [ordered]@{}
$script:notes = @()
$r.exe_mb = Mb (Get-Item $Exe).Length
if (-not $Zip) {
    $z = Get-ChildItem -ErrorAction SilentlyContinue (Join-Path $root 'dist\*.zip') | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($z) { $Zip = $z.FullName }
}
if ($Zip -and (Test-Path $Zip)) {
    $r.zip_mb = Mb (Get-Item $Zip).Length
    if ((Get-Item $Zip).LastWriteTime -lt (Get-Item $Exe).LastWriteTime) {
        $script:notes += "zip $([IO.Path]::GetFileName($Zip)) is older than brisk.exe (-Package rebuilds it)"
    }
} else {
    $r.zip_mb = $null
    $script:notes += "no zip in dist\ (-Package builds one)"
}

# Working sets of a process's descendants, by name: ty, ruff and the rest.
function Get-ChildMemory([int]$parent) {
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, WorkingSetSize)
    $ids = @{ $parent = $true }
    $m = @{ ty = 0.0; ruff = 0.0; other = 0.0 }
    do {
        $more = @($all | Where-Object { $ids.ContainsKey([int]$_.ParentProcessId) -and -not $ids.ContainsKey([int]$_.ProcessId) })
        foreach ($k in $more) {
            $ids[[int]$k.ProcessId] = $true
            $name = [IO.Path]::GetFileNameWithoutExtension($k.Name).ToLower()
            $key = if ($name -eq 'ty' -or $name -eq 'ruff') { $name } else { 'other' }
            $m[$key] += Mb ([double]$k.WorkingSetSize)
        }
    } while ($more.Count -gt 0)
    $m
}

function New-Launch([string]$dir, [string]$project, [hashtable]$envs) {
    New-Item -ItemType Directory -Force (Join-Path $dir 'appdata') | Out-Null
    $psi = New-Object System.Diagnostics.ProcessStartInfo $Exe
    $psi.Arguments = '"' + $project + '"'
    $psi.UseShellExecute = $false
    $psi.WorkingDirectory = $dir
    $psi.EnvironmentVariables['APPDATA'] = Join-Path $dir 'appdata'
    foreach ($k in $envs.Keys) { $psi.EnvironmentVariables[$k] = $envs[$k] }
    $psi
}

# One launch with the in-app probe (BRISK_BUDGET). Returns start_ms, the startup marks and,
# for the full launch, the idle report and the children sampled at that moment.
function Invoke-Start([bool]$full) {
    $tmp = Join-Path $env:TEMP ('brisk-budget-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $report = Join-Path $tmp 'report.json'
    $envs = @{ BRISK_BUDGET = $report }
    if (-not $full) { $envs.BRISK_BUDGET_STARTUP_ONLY = '1' }
    $p = [Diagnostics.Process]::Start((New-Launch $tmp $Project $envs))
    $start = [DateTimeOffset]::new($p.StartTime).ToUnixTimeMilliseconds()
    $res = @{ data = $null; kids = $null; marks = [ordered]@{} }
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
        if ($null -ne $res.kids -or -not (Test-Path $report)) { continue }
        try { $d = Get-Content -Raw $report | ConvertFrom-Json } catch { continue }
        if ($d.stage -eq 'idle') { $res.kids = Get-ChildMemory $p.Id }
    }
    if (-not $p.HasExited) {
        $script:notes += "a launch timed out"
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path $report) { $res.data = Get-Content -Raw $report | ConvertFrom-Json }
    if ($res.data -and $res.data.first_frame_unix_ms -gt 0) {
        $res.start_ms = [int]($res.data.first_frame_unix_ms - $start)
        if ($res.data.marks) {
            foreach ($m in $res.data.marks.PSObject.Properties) { $res.marks[$m.Name] = [int]($m.Value - $start) }
        }
        $res.marks['first_render'] = [int]($res.data.first_render_unix_ms - $start)
        $res.marks['first_frame'] = $res.start_ms
    }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    $res
}

# The same from outside, for an exe built without the probe (before BRISK_BUDGET existed):
# start_ms is process start -> main window created (the first frame follows within a few ms),
# idle is the working set 5 s later. The launch is then killed (children die with its Job).
function Invoke-StartExternal([bool]$full) {
    $tmp = Join-Path $env:TEMP ('brisk-budget-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $p = [Diagnostics.Process]::Start((New-Launch $tmp $Project @{}))
    $start = [DateTimeOffset]::new($p.StartTime).ToUnixTimeMilliseconds()
    $res = @{ data = $null; kids = $null; marks = [ordered]@{} }
    $deadline = (Get-Date).AddSeconds(60)
    while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
        $p.Refresh()
        if ($p.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 10
    }
    if (-not $p.HasExited -and $p.MainWindowHandle -ne [IntPtr]::Zero) {
        $res.start_ms = [int]([DateTimeOffset]::Now.ToUnixTimeMilliseconds() - $start)
        $res.marks['window_created'] = $res.start_ms
        if ($full) {
            Start-Sleep -Seconds 5
            $p.Refresh()
            $res.data = [pscustomobject]@{ idle_ws_mb = (Mb $p.WorkingSet64); idle_private_mb = (Mb $p.PrivateMemorySize64) }
            $res.kids = Get-ChildMemory $p.Id
        }
    }
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    $p.WaitForExit(5000) | Out-Null
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    $res
}

# Does the exe have the probe? (Its env var name is in the binary.)
function Test-Probe {
    $bytes = [IO.File]::ReadAllBytes($Exe)
    $text = [Text.Encoding]::ASCII.GetString($bytes)
    $text.Contains('BRISK_BUDGET_STARTUP_ONLY')
}

function Median($xs) {
    $s = @($xs | Sort-Object)
    $s[[int][math]::Floor(($s.Count - 1) / 2)]
}

# 1. Startup: -Runs launches; the last one also stays 5 s for the idle working set.
$starts = @()
$marks = [ordered]@{}
$last = $null
if ($SizeOnly) {
    $Runs = 0
    $NoMembudget = $true
    foreach ($k in 'start_ms', 'idle_mb', 'membudget_mb') { $Limits.Remove($k) }
}
$probe = Test-Probe
if (-not $probe) { $script:notes += "this exe has no BRISK_BUDGET probe: start_ms is process start -> main window created, idle measured from outside, no startup profile" }
for ($i = 1; $i -le $Runs; $i++) {
    $run = if ($probe) { Invoke-Start ($i -eq $Runs) } else { Invoke-StartExternal ($i -eq $Runs) }
    if ($null -eq $run.start_ms) { Write-Host "Launch $i did not report a first frame"; exit 2 }
    Write-Host ("launch {0}: {1} {2} ms" -f $i, $(if ($probe) { 'first frame' } else { 'window created' }), $run.start_ms)
    $starts += $run.start_ms
    foreach ($k in $run.marks.Keys) {
        if (-not $marks.Contains($k)) { $marks[$k] = @() }
        $marks[$k] += $run.marks[$k]
    }
    if ($i -eq 1) { $r.start_cold_ms = $run.start_ms }
    $last = $run
}
if (-not $SizeOnly) {
    $r.start_ms = Median $starts
    $r.idle_mb = $last.data.idle_ws_mb
    $r.idle_private_mb = $last.data.idle_private_mb
    if ($last.kids) { $r.idle_children_mb = [math]::Round($last.kids.ty + $last.kids.ruff + $last.kids.other, 1) }
}

# 2. MEMBUDGET on a copy of the project (its own verdict is switched off with a huge budget:
#    the limit is checked here); ty and ruff are sampled while it runs.
if (-not $NoMembudget) {
    $tmp = Join-Path $env:TEMP ('brisk-budget-mb-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    $proj = Join-Path $tmp 'project'
    New-Item -ItemType Directory -Force $tmp | Out-Null
    Copy-Item -Recurse $Project $proj
    $psi = New-Launch $tmp $proj @{ BRISK_SMOKE_MEMBUDGET = '100000' }
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardOutput = $true
    $p = [Diagnostics.Process]::Start($psi)
    $errTask = $p.StandardError.ReadToEndAsync()
    $outTask = $p.StandardOutput.ReadToEndAsync()
    $peak = @{ ty = 0.0; ruff = 0.0 }
    $deadline = (Get-Date).AddSeconds(240)
    while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
        $k = Get-ChildMemory $p.Id
        foreach ($n in 'ty', 'ruff') { if ($k[$n] -gt $peak[$n]) { $peak[$n] = $k[$n] } }
        Start-Sleep -Milliseconds 500
    }
    if (-not $p.HasExited) {
        $script:notes += "MEMBUDGET timed out"
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
    $p.WaitForExit()
    $lines = ($errTask.Result + $outTask.Result) -split "`r?`n"
    $line = $lines | Where-Object { $_ -match 'smoke-membudget: working set' } | Select-Object -Last 1
    if ($line -match 'working set (\d+) MB, private (\d+) MB.*live children (\S+), total ([\d.]+\w+)') {
        $r.membudget_mb = [int]$Matches[1]
        $r.membudget_private_mb = [int]$Matches[2]
        $r.membudget_children = $Matches[3]
        $r.membudget_time = $Matches[4]
    } else {
        $r.membudget_mb = $null
        $script:notes += "MEMBUDGET printed no result; its log tail:`n" + (($lines | Select-Object -Last 8) -join "`n")
    }
    foreach ($l in $lines | Where-Object { $_ -match 'smoke-membudget: (run finished|\d+ tabs)' }) { $script:notes += $l.Trim() }
    $r.ty_peak_mb = $peak.ty
    $r.ruff_peak_mb = $peak.ruff
    if ($peak.ty -eq 0) { $script:notes += "ty did not run during MEMBUDGET (not next to the interpreter or on PATH?)" }
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$over = @()
$rows = foreach ($key in $r.Keys) {
    $lim = $Limits[$key]
    $val = $r[$key]
    $status = ''
    if ($null -ne $lim) {
        if ($null -eq $val) { $status = 'n/a' }
        elseif ([double]$val -gt [double]$lim) { $status = 'OVER'; $over += $key }
        else { $status = 'ok' }
    }
    [pscustomobject]@{ Metric = $key; Value = $val; Budget = $lim; Status = $status }
}
$rows | Format-Table -AutoSize | Out-String -Width 200 | Write-Host

# Where the start goes: each moment's median since process start and the step from the previous.
$meds = foreach ($k in $marks.Keys) { [pscustomobject]@{ Name = $k; Med = (Median $marks[$k]) } }
$prev = 0
$prof = foreach ($m in ($meds | Sort-Object Med)) {
    [pscustomobject]@{ Startup = $m.Name; 'ms since start' = $m.Med; 'step ms' = $m.Med - $prev }
    $prev = $m.Med
}
$prof | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
foreach ($n in $script:notes) { Write-Host "note: $n" }
if ($Out) {
    $startup = [ordered]@{}
    foreach ($s in $prof) { $startup[$s.Startup] = $s.'ms since start' }
    $r.over = $over
    $r.startup = $startup
    $r.limits = $Limits
    $r | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $Out
}
if ($over.Count -gt 0) {
    Write-Host ("OVER BUDGET: " + ($over -join ', '))
    exit 1
}
Write-Host 'Within budget'
exit 0
