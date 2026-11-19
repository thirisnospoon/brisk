# Writes THIRD-PARTY-NOTICES.txt: the licenses of everything compiled into brisk.exe (the Rust
# crates brisk-app depends on at run time for Windows x64, and the Lucide icons). The portable
# package ships it next to brisk.exe. Rerun after changing dependencies.
# Usage (from the repository root): powershell -ExecutionPolicy Bypass -File scripts\third-party-notices.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$json = cargo metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$meta = ($json -join "`n") | ConvertFrom-Json

$pkgs = @{}; foreach ($p in $meta.packages) { $pkgs[$p.id] = $p }
$nodes = @{}; foreach ($n in $meta.resolve.nodes) { $nodes[$n.id] = $n }
$members = @{}; foreach ($m in $meta.workspace_members) { $members[$m] = $true }

# Run-time dependencies only: dev- and build-dependencies are not part of the executable.
$app = ($meta.packages | Where-Object { $_.name -eq 'brisk-app' }).id
$seen = @{ $app = $true }
$queue = New-Object System.Collections.Queue
$queue.Enqueue($app)
while ($queue.Count -gt 0) {
    $id = $queue.Dequeue()
    foreach ($d in $nodes[$id].deps) {
        $normal = @($d.dep_kinds | Where-Object { $null -eq $_.kind }).Count -gt 0
        if ($normal -and -not $seen.ContainsKey($d.pkg)) { $seen[$d.pkg] = $true; $queue.Enqueue($d.pkg) }
    }
}
$deps = $seen.Keys | Where-Object { -not $members.ContainsKey($_) } | ForEach-Object { $pkgs[$_] } |
    Sort-Object name, version

$out = New-Object System.Text.StringBuilder
function Add([string]$s) { [void]$out.Append($s.Replace("`r`n", "`n").TrimEnd() + "`n") }
$rule = '=' * 78

Add "Third-party software in Brisk"
Add ""
Add "brisk.exe contains the components listed below. Each is used under its own license,"
Add "reproduced in full. Where several components carry an identical license file, the text"
Add "is printed once and later entries refer back to it."
Add ""
Add "ty and ruff (lib\ty.exe, lib\ruff.exe) and debugpy (lib\debugpy) come with their own"
Add "license files in the lib folder."
Add ""
Add $rule
Add "Lucide icons (https://lucide.dev)"
Add $rule
Add (Get-Content -Raw -Encoding UTF8 'crates\brisk-app\assets\icons\LICENSE-lucide.txt')
Add ""

$texts = @{}
$missing = @()
foreach ($p in $deps) {
    $dir = Split-Path -Parent $p.manifest_path
    $files = @(Get-ChildItem -LiteralPath $dir -File |
        Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE|COPYRIGHT)' } | Sort-Object Name)
    if ($files.Count -eq 0 -and $p.license_file) {
        $f = Join-Path $dir $p.license_file
        if (Test-Path -LiteralPath $f) { $files = @(Get-Item -LiteralPath $f) }
    }
    Add $rule
    $title = "$($p.name) $($p.version)"
    Add $title
    if ($p.license) { Add "License: $($p.license)" }
    if ($p.repository) { Add "Source: $($p.repository)" }
    Add $rule
    if ($files.Count -eq 0) {
        $missing += $title
        if ($p.authors) { Add ("Copyright (c) " + ($p.authors -join ', ')) }
        Add "The published crate has no license file. The standard text of the license named"
        Add "above applies; the MIT and Apache-2.0 texts are at the end of this file."
    }
    foreach ($f in $files) {
        $text = (Get-Content -Raw -Encoding UTF8 -LiteralPath $f.FullName).Replace("`r`n", "`n").Trim()
        if ($texts.ContainsKey($text)) {
            Add "$($f.Name): same text as $($texts[$text])."
        } else {
            $texts[$text] = "$($f.Name) of $title"
            Add "--- $($f.Name) ---"
            Add $text
        }
    }
    Add ""
}

Add $rule
Add "Standard license texts"
Add $rule
Add "--- MIT License ---"
Add @"
Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"@
Add ""
Add "--- Apache License 2.0 ---"
Add (Get-Content -Raw -Encoding UTF8 'LICENSE-APACHE')

$utf8 = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText((Join-Path $root 'THIRD-PARTY-NOTICES.txt'), $out.ToString(), $utf8)
"THIRD-PARTY-NOTICES.txt: $($deps.Count) crates, $($texts.Count) distinct license texts"
if ($missing.Count) { "Without a license file: " + ($missing -join ', ') }
