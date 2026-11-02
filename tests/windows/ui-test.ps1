# UI and integration tests of the Windows edition, on the runner's desktop:
#
#   tests/windows/ui-test.ps1 -Exe <WeekNumber.exe> -Shots <screenshot folder>
#
# A fresh user profile folder each run. The app starts as a user starts it,
# opens its calendar and every dialog, is browsed and closed with the
# keyboard, and hands over between launches; every step is checked and each
# window is saved as a screenshot. Writes a Markdown table of results to
# $env:GITHUB_STEP_SUMMARY and exits non-zero if a test failed.
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Shots
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class W {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int size);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool SystemParametersInfo(uint a, uint b, string c, uint d);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);

    // A left click at a screen point (gives that window the keyboard).
    public static void Click(int x, int y) {
        SetCursorPos(x, y);
        mouse_event(2, 0, 0, 0, UIntPtr.Zero);
        mouse_event(4, 0, 0, 0, UIntPtr.Zero);
    }

    // Visible top-level windows of a process: handle and title.
    public static List<KeyValuePair<IntPtr, string>> Windows(uint pid) {
        var found = new List<KeyValuePair<IntPtr, string>>();
        EnumWindows((h, l) => {
            uint p;
            GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) {
                var sb = new StringBuilder(256);
                GetWindowText(h, sb, 256);
                found.Add(new KeyValuePair<IntPtr, string>(h, sb.ToString()));
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    // The window's visible bounds (without the invisible resize border).
    public static RECT Bounds(IntPtr h) {
        RECT r;
        if (DwmGetWindowAttribute(h, 9, out r, Marshal.SizeOf(typeof(RECT))) != 0) GetWindowRect(h, out r);
        return r;
    }

    [DllImport("user32.dll")] static extern uint MapVirtualKey(uint code, uint type);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint msg, IntPtr wp, IntPtr lp);

    // Escape, delivered to the window itself: the Windows 11 ARM64 runner
    // drops synthesized Escape presses before they reach any window.
    public static void Escape(IntPtr h) {
        PostMessage(h, 0x0100, (IntPtr)0x1B, (IntPtr)0x00010001);
        PostMessage(h, 0x0101, (IntPtr)0x1B, unchecked((IntPtr)(int)0xC0010001));
    }

    // A key press with its scan code, as a keyboard sends it.
    public static void Key(byte vk) {
        byte scan = (byte)MapVirtualKey(vk, 0);
        uint ext = (vk >= 0x21 && vk <= 0x28) ? 1u : 0u;  // arrows, Home: extended keys
        keybd_event(vk, scan, ext, UIntPtr.Zero);
        System.Threading.Thread.Sleep(30);
        keybd_event(vk, scan, ext | 2, UIntPtr.Zero);
    }

    public static bool Wallpaper(string path) { return SystemParametersInfo(20, 0, path, 3); }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);

    // Hide every visible top-level window but the taskbar and the desktop
    // (the runner's console, system dialogs), so screenshots show only
    // WeekNumber on the wallpaper.
    public static int HideOthers() {
        var keep = new HashSet<string> { "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW" };
        var hide = new List<IntPtr>();
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            var sb = new StringBuilder(256);
            GetClassName(h, sb, 256);
            if (!keep.Contains(sb.ToString())) hide.Add(h);
            return true;
        }, IntPtr.Zero);
        foreach (var h in hide) ShowWindow(h, 0);
        return hide.Count;
    }
}
"@

$Exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $Shots | Out-Null
$Shots = (Resolve-Path $Shots).Path
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$rows = [System.Collections.Generic.List[string]]::new()
$script:failed = $false

function Check([string] $name, [scriptblock] $test) {
    $ok = $false
    try { $ok = [bool](& $test) } catch { Write-Host "  $($_.Exception.Message)" }
    if ($ok) { $rows.Add("| $name | passed |"); Write-Host "PASS  $name" }
    else { $rows.Add("| $name | **FAILED** |"); Write-Host "FAIL  $name"; $script:failed = $true }
}

function WaitFor([scriptblock] $test, [int] $seconds = 10) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        try { if (& $test) { return $true } } catch {}
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Window([int] $procId, [string] $title) {
    [W]::Windows($procId) | Where-Object { $_.Value -eq $title } | Select-Object -First 1
}

$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
function Shot([string] $name, [int] $x, [int] $y, [int] $w, [int] $h) {
    $x = [math]::Max(0, $x); $y = [math]::Max(0, $y)
    $w = [math]::Min($w, $screen.Width - $x); $h = [math]::Min($h, $screen.Height - $y)
    $bmp = [System.Drawing.Bitmap]::new($w, $h)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $Shots "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# A window with room for its shadow.
function ShotWindow([string] $name, [IntPtr] $h) {
    $r = [W]::Bounds($h); $m = 24
    Shot $name ($r.L - $m) ($r.T - $m) ($r.R - $r.L + 2 * $m) ($r.B - $r.T + 2 * $m)
}

# The flyout with the taskbar below it.
function ShotFlyout([string] $name, [IntPtr] $h) {
    $r = [W]::Bounds($h); $m = 16
    Shot $name ($r.L - $m) ($r.T - $m) ($r.R - $r.L + 2 * $m) ($screen.Height - $r.T + $m)
}

function Hash([IntPtr] $h) {
    $r = [W]::Bounds($h)
    $bmp = [System.Drawing.Bitmap]::new([int]($r.R - $r.L), [int](($r.B - $r.T) * 0.7))
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
    [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($ms.ToArray()))
}

# The settings the window tests start with (as tests/settings.sh): public
# holidays of the Netherlands and Germany and a second clock for Tokyo.
function WriteSettings([string] $theme) {
    $dark = if ($theme -eq "dark") { "true" } else { "false" }
    @"
{
    "SelectedColor": "#FFFFFF",
    "IsBold": true,
    "HolidayCountry": "NL",
    "HolidayCountryPrimary": "NL",
    "HolidayCountrySecondary": "DE",
    "DarkMode": $dark,
    "StartupInitialized": true,
    "TimeZoneEnabled": true,
    "TimeZoneKey": "Tokyo Standard Time"
}
"@ | Out-File -FilePath $settings -Encoding ascii
}

function Alive([int] $procId) { [bool](Get-Process -Id $procId -ErrorAction SilentlyContinue) }

# A fresh profile and the same wallpaper as the other platforms' tests.
$env:LOCALAPPDATA = Join-Path $env:RUNNER_TEMP "ui-localappdata"
Remove-Item -Recurse -Force $env:LOCALAPPDATA -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $env:LOCALAPPDATA | Out-Null
$settings = Join-Path $env:LOCALAPPDATA "WeekNumber\settings.json"
$wallpaper = Join-Path $env:RUNNER_TEMP "wallpaper.png"
python (Join-Path $here "..\wallpaper.py") $wallpaper $screen.Width $screen.Height
[W]::Wallpaper($wallpaper) | Out-Null
# Out of the way: the runner's console and any other window.
"Hid $([W]::HideOthers()) other windows"
Start-Sleep -Seconds 1
Remove-Item Env:WEEKNUMBER_OPEN -ErrorAction SilentlyContinue

function Launch([string] $open) {
    if ($open) { $env:WEEKNUMBER_OPEN = $open } else { Remove-Item Env:WEEKNUMBER_OPEN -ErrorAction SilentlyContinue }
    $p = Start-Process -FilePath $Exe -PassThru
    Remove-Item Env:WEEKNUMBER_OPEN -ErrorAction SilentlyContinue
    $p
}

# --- First run ---------------------------------------------------------------
$p = Launch ""
Start-Sleep -Seconds 6
Check "First run: stays running" { -not $p.HasExited }
Check "Process: one process, no child processes" {
    @(Get-CimInstance Win32_Process -Filter "Name = 'WeekNumber.exe'").Count -eq 1 -and
    @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($p.Id)").Count -eq 0
}
Check "Settings: written on first run" { (Get-Content $settings -Raw | ConvertFrom-Json).StartupInitialized -eq $true }
Check "Run at Startup: off on first run" {
    -not (Get-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name WeekNumber -ErrorAction SilentlyContinue)
}
$proc = Get-Process -Id $p.Id
$idleMb = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
$wsMb = [math]::Round($proc.WorkingSet64 / 1MB, 1)

# --- The calendar --------------------------------------------------------------
# The pointer rests at the notification area, as after a click on the icon.
[W]::SetCursorPos($screen.Width - 120, $screen.Height - 20) | Out-Null
$old = $p
$p = Launch "calendar"
Check "Single instance: a new launch takes over from the running copy" { WaitFor { -not (Alive $old.Id) } 8 }
if (WaitFor { Window $p.Id "WeekNumber" } 10) {
    Start-Sleep -Seconds 1
    $cal = (Window $p.Id "WeekNumber").Key
    Check "Calendar: opens" { $true }
    $r = [W]::Bounds($cal)
    Check "Calendar: opens above the taskbar, at the notification area" {
        $r.B -le [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea.Bottom -and $r.R -gt $screen.Width * 3 / 4
    }
    ShotFlyout "windows-calendar-light" $cal
    $proc = Get-Process -Id $p.Id
    $openMb = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
    # The keyboard goes to the calendar (a click on its empty top corner).
    [W]::Click($r.L + 8, $r.T + 6)
    Start-Sleep -Milliseconds 400
    $before = Hash $cal
    [W]::Key(0x27)  # Right
    Start-Sleep -Milliseconds 600
    Check "Calendar: the arrow keys browse the months" { (Hash $cal) -ne $before }
    [W]::Key(0x25)  # Left
    Start-Sleep -Milliseconds 400
    [W]::Escape($cal)
    Check "Calendar: Escape closes it" { WaitFor { -not (Window $p.Id "WeekNumber") } 5 }
} else {
    Check "Calendar: opens" { $false }
}

# --- Every window, light and dark: each launch hands over ----------------------
$windows = [ordered]@{
    "calendar" = "WeekNumber"
    "color"    = "WeekNumber $([char]0x2013) Icon Color"
    "holidays" = "WeekNumber $([char]0x2013) Public Holidays"
    "timezone" = "WeekNumber $([char]0x2013) Time Zone"
    "about"    = "About WeekNumber"
    "legal"    = "WeekNumber $([char]0x2013) Legal notices"
}
foreach ($theme in "dark", "light") {
    WriteSettings $theme
    foreach ($what in $windows.Keys) {
        if ($theme -eq "light" -and $what -eq "legal") { continue }
        $title = $windows[$what]
        [W]::SetCursorPos($screen.Width - 120, $screen.Height - 20) | Out-Null
        $old = $p
        $p = Launch $what
        $shown = WaitFor { Window $p.Id $title } 10
        Check "Window ($theme): $title opens" { $shown }
        if ($shown) {
            Start-Sleep -Milliseconds 1200
            $h = (Window $p.Id $title).Key
            if ($what -eq "calendar") { ShotFlyout "windows-calendar-$theme-holidays" $h }
            else { ShotWindow "windows-$what-$theme" $h }
        }
        Check "Single instance: the $what launch took over" { WaitFor { -not (Alive $old.Id) } 8 }
    }
}

# --- The tray menu ----------------------------------------------------------------
[W]::SetCursorPos($screen.Width - 120, $screen.Height - 20) | Out-Null
$old = $p
$p = Launch "menu"
$menuShown = WaitFor { @([W]::Windows($p.Id)).Count -gt 0 } 10
Check "Tray menu: opens" { $menuShown }
if ($menuShown) {
    Start-Sleep -Milliseconds 800
    $m = ([W]::Windows($p.Id) | Select-Object -First 1).Key
    ShotFlyout "windows-menu-light" $m
    $mr = [W]::Bounds($m)
    [W]::Escape($m)
    Check "Tray menu: Escape closes it" { WaitFor { @([W]::Windows($p.Id)).Count -eq 0 } 5 }
}
Start-Sleep -Seconds 1
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

Check "Memory: idle below 20 MB (private)" { $idleMb -lt 20 }

$summary = @(
    "### UI tests (Windows $env:PROCESSOR_ARCHITECTURE, $($screen.Width)x$($screen.Height))",
    "",
    "| Test | Result |",
    "|---|---|"
) + $rows + @(
    "",
    "| Idle private (MB) | Idle working set (MB) | Calendar open, private (MB) | Size (bytes) |",
    "|---|---|---|---|",
    "| $idleMb | $wsMb | $openMb | $((Get-Item $Exe).Length) |"
)
if ($env:GITHUB_STEP_SUMMARY) { $summary | Out-File -FilePath $env:GITHUB_STEP_SUMMARY -Append -Encoding utf8 }
$summary | Write-Host
if ($script:failed) { exit 1 }
