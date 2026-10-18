# Drives the DAW window: screenshot, click, double-click, key presses.
# Usage: ui-drive.ps1 shot <out.png> | click <x> <y> | dclick <x> <y> | key <vk> | front
#        down <x> <y> | move <x> <y> | up <x> <y>  (a drag in steps, to look mid-drag)
#        keydown <vk> | keyup <vk>  (hold a key, e.g. 17 Ctrl, 18 Alt, 16 Shift)
# Coordinates are relative to the window's client area top-left.
param([string]$cmd, [string]$a, [string]$b)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct PT { public int X, Y; }
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref PT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[W]::SetProcessDPIAware() | Out-Null
# DAW_PID picks one copy when several run (e.g. Claude's beside the user's).
if ($env:DAW_PID) {
    $p = Get-Process -Id $env:DAW_PID -ErrorAction Stop
} else {
    $p = Get-Process daw -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
}
$h = $p.MainWindowHandle
[W]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 150
$r = New-Object W+RECT; [W]::GetClientRect($h, [ref]$r) | Out-Null
$o = New-Object W+PT; [W]::ClientToScreen($h, [ref]$o) | Out-Null
function MoveTo($x, $y) { [W]::SetCursorPos($o.X + [int]$x, $o.Y + [int]$y) | Out-Null; Start-Sleep -Milliseconds 60 }
function Click { [W]::mouse_event(2,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [W]::mouse_event(4,0,0,0,[UIntPtr]::Zero) }
switch ($cmd) {
  "shot" {
    $w = $r.R; $hh = $r.B
    $bmp = New-Object System.Drawing.Bitmap $w, $hh
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($o.X, $o.Y, 0, 0, $bmp.Size)
    $bmp.Save($a, [System.Drawing.Imaging.ImageFormat]::Png)
    "saved ${w}x${hh}"
  }
  "click" { MoveTo $a $b; Click }
  "dclick" { MoveTo $a $b; Click; Start-Sleep -Milliseconds 60; Click }
  "move" { MoveTo $a $b }
  "down" { MoveTo $a $b; [W]::mouse_event(2,0,0,0,[UIntPtr]::Zero) }
  "keydown" { [W]::keybd_event([byte][int]$a,0,0,[UIntPtr]::Zero) }
  "keyup" { [W]::keybd_event([byte][int]$a,0,2,[UIntPtr]::Zero) }
  "up" { MoveTo $a $b; [W]::mouse_event(4,0,0,0,[UIntPtr]::Zero) }
  "key" { [W]::keybd_event([byte][int]$a,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [W]::keybd_event([byte][int]$a,0,2,[UIntPtr]::Zero) }
  "front" { "focused" }
  "type" { Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait($a) }
  "rclick" { MoveTo $a $b; [W]::mouse_event(8,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [W]::mouse_event(16,0,0,0,[UIntPtr]::Zero) }
  "drag" {
    $to = $b.Split(","); $from = $a.Split(",")
    MoveTo $from[0] $from[1]; [W]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
    for ($i = 1; $i -le 10; $i++) {
      $x = [int]$from[0] + ([int]$to[0] - [int]$from[0]) * $i / 10
      $y = [int]$from[1] + ([int]$to[1] - [int]$from[1]) * $i / 10
      MoveTo $x $y
    }
    [W]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
  }
}
