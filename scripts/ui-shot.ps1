param([string]$Exe, [string]$Project, [string]$AppData, [string]$OutDir, [string]$Steps, [string]$Env = "", [int]$StartWait = 4000)
# $Exe has to be a build with the smoke feature, target/smoke/brisk.exe (docs/testing.md).
# Starts brisk on $Project (optionally with "NAME=value,NAME2=value" in $Env), waits $StartWait ms,
# then runs $Steps against its window like debug-mouse-smoke.ps1: "shot:name; click:x,y; dbl:x,y;
# rclick:x,y; key:vk; skey:vk; mkey:SCA,vk; char:text; move:x,y; drag:x1,y1,x2,y2; wait:ms".
# Shots are PrintWindow captures of the whole window, for checking layout pixel by pixel.
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr p);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll")] public static extern bool GetKeyboardState(byte[] s);
  [DllImport("user32.dll")] public static extern bool SetKeyboardState(byte[] s);
  public struct RECT { public int L, T, R, B; }
  public struct POINT { public int X, Y; }
}
"@
[void][W]::SetProcessDPIAware()
$env:APPDATA = $AppData
foreach ($kv in ($Env -split ',')) { if ($kv) { $n, $v = $kv -split '=', 2; Set-Item -Path "env:$n" -Value $v } }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$log = Join-Path $OutDir "log.txt"
$p = Start-Process -FilePath $Exe -ArgumentList "`"$Project`"" -PassThru -RedirectStandardError $log
$h = [IntPtr]0
for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]0; $i++) { Start-Sleep -Milliseconds 500; $p.Refresh(); $h = $p.MainWindowHandle }
if ($h -eq [IntPtr]0) { Stop-Process -Id $p.Id -Force; throw "no window" }
Start-Sleep -Milliseconds $StartWait

function Shot($name) {
  $r = New-Object W+RECT
  [void][W]::GetWindowRect($h, [ref]$r)
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $dc = $g.GetHdc()
  [void][W]::PrintWindow($h, $dc, 3)
  $g.ReleaseHdc($dc)
  $bmp.Save((Join-Path $OutDir "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}
function L($x, $y) { [IntPtr](($y -shl 16) -bor ($x -band 0xffff)) }
function MMove($x, $y) { [void][W]::PostMessage($h, 0x200, [IntPtr]0, (L $x $y)) }
function Click($x, $y) { MMove $x $y; Start-Sleep -Milliseconds 60; [void][W]::PostMessage($h, 0x201, [IntPtr]1, (L $x $y)); Start-Sleep -Milliseconds 60; [void][W]::PostMessage($h, 0x202, [IntPtr]0, (L $x $y)) }
function RClick($x, $y) { MMove $x $y; Start-Sleep -Milliseconds 60; [void][W]::PostMessage($h, 0x204, [IntPtr]2, (L $x $y)); Start-Sleep -Milliseconds 60; [void][W]::PostMessage($h, 0x205, [IntPtr]0, (L $x $y)) }
function Mark($text) { Add-Content (Join-Path $OutDir "steps.txt") ("{0:HH:mm:ss.fff} {1}" -f (Get-Date), $text) }

$c = New-Object W+RECT
[void][W]::GetClientRect($h, [ref]$c)
Mark "client $($c.R)x$($c.B)"
foreach ($s in ($Steps -split ';')) {
  $s = $s.Trim(); if (-not $s) { continue }
  $k, $v = $s -split ':', 2
  Mark $s
  switch ($k) {
    'shot' { Shot $v }
    'wait' { Start-Sleep -Milliseconds ([int]$v) }
    'click' { $x, $y = $v -split ','; Click ([int]$x) ([int]$y) }
    'dbl' { $x, $y = $v -split ','; Click ([int]$x) ([int]$y); Start-Sleep -Milliseconds 30; Click ([int]$x) ([int]$y) }
    'rclick' { $x, $y = $v -split ','; RClick ([int]$x) ([int]$y) }
    'drag' {
      # Press at the first point, move with the button held in 6 steps, release at the second.
      $x1, $y1, $x2, $y2 = ($v -split ',') | ForEach-Object { [int]$_ }
      MMove $x1 $y1; Start-Sleep -Milliseconds 60
      [void][W]::PostMessage($h, 0x201, [IntPtr]1, (L $x1 $y1)); Start-Sleep -Milliseconds 80
      for ($i = 1; $i -le 6; $i++) { $x = $x1 + [int](($x2 - $x1) * $i / 6); $y = $y1 + [int](($y2 - $y1) * $i / 6); [void][W]::PostMessage($h, 0x200, [IntPtr]1, (L $x $y)); Start-Sleep -Milliseconds 60 }
      Start-Sleep -Milliseconds 200
      [void][W]::PostMessage($h, 0x202, [IntPtr]0, (L $x2 $y2))
    }
    'move' { $x, $y = $v -split ','; MMove ([int]$x) ([int]$y) }
    'char' { foreach ($ch in $v.ToCharArray()) { [void][W]::PostMessage($h, 0x102, [IntPtr][int]$ch, [IntPtr]1); Start-Sleep -Milliseconds 40 } }
    { $_ -in 'skey', 'mkey' } {
      # A key with modifiers held: they are set in the key state shared with brisk's thread
      # while the key is posted. skey:vk = Shift; mkey:SCA,vk = any of Shift, Ctrl, Alt.
      if ($k -eq 'skey') { $mods = 'S'; $vk = $v } else { $mods, $vk = $v -split ',' }
      $vks = @(); if ($mods -match 'S') { $vks += 0x10, 0xA0 }; if ($mods -match 'C') { $vks += 0x11, 0xA2 }; if ($mods -match 'A') { $vks += 0x12, 0xA4 }
      $me = [W]::GetCurrentThreadId(); $them = [W]::GetWindowThreadProcessId($h, [IntPtr]0)
      [void][W]::AttachThreadInput($me, $them, $true)
      $ks = New-Object byte[] 256; [void][W]::GetKeyboardState($ks); foreach ($m in $vks) { $ks[$m] = 0x80 }; [void][W]::SetKeyboardState($ks)
      [void][W]::PostMessage($h, 0x100, [IntPtr]([int]$vk), [IntPtr]1); Start-Sleep -Milliseconds 40; [void][W]::PostMessage($h, 0x101, [IntPtr]([int]$vk), [IntPtr]0xC0000001)
      Start-Sleep -Milliseconds 300
      foreach ($m in $vks) { $ks[$m] = 0 }; [void][W]::SetKeyboardState($ks)
      [void][W]::AttachThreadInput($me, $them, $false)
    }
    'key' { [void][W]::PostMessage($h, 0x100, [IntPtr]([int]$v), [IntPtr]1); Start-Sleep -Milliseconds 40; [void][W]::PostMessage($h, 0x101, [IntPtr]([int]$v), [IntPtr]0xC0000001) }
  }
  Start-Sleep -Milliseconds 300
}
Stop-Process -Id $p.Id -Force
