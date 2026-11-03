# Shared helpers for the desktop hand tests (dot-source; Windows PowerShell 5.1, interactive session).
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Drawing, System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class Tok {
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("advapi32.dll")] public static extern bool OpenProcessToken(IntPtr p, uint access, out IntPtr t);
    [DllImport("advapi32.dll")] public static extern bool GetTokenInformation(IntPtr t, int cls, out int info, int len, out int ret);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    // 1 elevated, 0 not, -1 could not open (an elevated process seen from a limited one)
    public static int Elevated(int pid) {
        IntPtr p = OpenProcess(0x1000, false, pid);
        if (p == IntPtr.Zero) return -1;
        IntPtr t; int e = 0, r;
        if (!OpenProcessToken(p, 8, out t)) { CloseHandle(p); return -1; }
        GetTokenInformation(t, 20, out e, 4, out r);
        CloseHandle(t); CloseHandle(p);
        return e != 0 ? 1 : 0;
    }
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
'@
$script:A = [Windows.Automation.AutomationElement]
$script:root = $A::RootElement
function Log($s) { Add-Content $LogFile "$(Get-Date -f HH:mm:ss) $s" }
function Shot($name) {
    $b = [Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object Drawing.Bitmap $b.Width, $b.Height
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $ShotDir "$name.png"), [Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}
function Windows-Of($procId) {
    $c = New-Object Windows.Automation.PropertyCondition($A::ProcessIdProperty, $procId)
    $root.FindAll([Windows.Automation.TreeScope]::Children, $c)
}
# The first element named $name (a button, by default) in any top-level window of $procId, waiting up to $secs.
function Find-Named($procId, $name, $secs = 30) {
    $c = New-Object Windows.Automation.PropertyCondition($A::NameProperty, $name)
    for ($i = 0; $i -lt $secs * 2; $i++) {
        foreach ($w in (Windows-Of $procId)) {
            $e = $w.FindFirst([Windows.Automation.TreeScope]::Descendants, $c)
            if ($e) { return $e }
        }
        Start-Sleep -Milliseconds 500
    }
    $null
}
# Presses button $id of the dialog holding element $e by message (works on a locked desktop):
# TDM_CLICK_BUTTON for a task dialog, WM_COMMAND for a classic dialog or message box.
function Press($e, [int]$id) {
    $walker = [Windows.Automation.TreeWalker]::ControlViewWalker
    $w = $e
    while ($w -and -not ($w.Current.ClassName -eq '#32770' -and $w.Current.NativeWindowHandle)) { $w = $walker.GetParent($w) }
    if (-not $w) { return $false }
    $h = [IntPtr]$w.Current.NativeWindowHandle
    [Tok]::PostMessage($h, 0x466, [IntPtr]$id, [IntPtr]::Zero) | Out-Null
    Start-Sleep -Milliseconds 700
    try { $null = $w.Current.Name; [Tok]::PostMessage($h, 0x111, [IntPtr]$id, [IntPtr]::Zero) | Out-Null } catch {}
    $true
}
function Window-Text($procId) {
    $out = @()
    foreach ($w in (Windows-Of $procId)) {
        $out += "[$($w.Current.ClassName)] $($w.Current.Name)"
        foreach ($e in $w.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
            if ($e.Current.Name) { $out += "  $($e.Current.ControlType.ProgrammaticName): $($e.Current.Name)" }
        }
    }
    $out -join "`n"
}

# Every top-level window of a process, hidden ones too (EnumWindows)
Add-Type @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Wins {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    public static List<string> Of(uint want) {
        var r = new List<string>();
        EnumWindows((h, l) => {
            uint pid; uint tid = GetWindowThreadProcessId(h, out pid);
            if (pid == want) {
                var t = new StringBuilder(256); var c = new StringBuilder(256);
                GetWindowText(h, t, 256); GetClassName(h, c, 256);
                r.Add(String.Format("{0:X} tid {1} visible {2} [{3}] {4}", h.ToInt64(), tid, IsWindowVisible(h), c, t));
            }
            return true;
        }, IntPtr.Zero);
        return r;
    }
}
'@
function Main-Hwnd($procId) {
    foreach ($w in [Wins]::Of([uint32]$procId)) { if ($w -match '^([0-9A-F]+) .*\[PaTorrentMainWnd(\.[0-9a-f]{16})?\]') { return [IntPtr][Convert]::ToInt64($matches[1], 16) } }
    [IntPtr]::Zero
}
