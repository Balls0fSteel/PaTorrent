# D42 wizard checks in user x's console session: the setup refuses a portable folder on its
# Directory page (-Part setup); the uninstaller's "Remove settings" takes profiles\ and
# rookparcel.lock too (-Part uninstall, after a silent per-user install). From the elevated SSH login:
#   in-session.ps1 -Script C:\Users\x\patest\wizard-profiles.ps1 -ScriptArgs "-Part setup"
# It drives the 1.1.50 setup in C:\Users\x\patest; change the file name for another version.
param([string]$Part = 'setup')
$LogFile = "C:\Users\x\patest\wizard-$Part.log"; $ShotDir = 'C:\Users\x\patest\shots'
New-Item -ItemType Directory -Force $ShotDir | Out-Null
Remove-Item $LogFile -ErrorAction SilentlyContinue
. C:\Users\x\patest\uia.ps1
function Pid-Of($name) { for ($i = 0; $i -lt 60; $i++) { $p = Get-Process $name -ErrorAction SilentlyContinue | Select-Object -First 1; if ($p) { return $p.Id }; Start-Sleep -Milliseconds 500 }; 0 }
function Next($procId, $label = 'Next >') {
    $b = Find-Named $procId $label 20
    if (-not $b) { Log "FAIL no '$label'`n$(Window-Text $procId)"; return $false }
    Press $b 1 | Out-Null; Start-Sleep 1; $true
}

if ($Part -eq 'setup') {
    $target = 'C:\Users\x\patest\portable-wizard'
    New-Item -ItemType Directory -Force $target | Out-Null
    Set-Content -Encoding ascii "$target\rookparcel.ini" ''
    $p = Start-Process 'C:\Users\x\patest\rookparcel-1.1.50-windows-x64-setup.exe' -ArgumentList '/LANG=1033' -PassThru
    Next $p.Id | Out-Null                 # Welcome
    Next $p.Id 'I Agree' | Out-Null        # License
    Next $p.Id | Out-Null                 # Users
    Next $p.Id | Out-Null                 # Components
    # the folder box shows the default folder; on a locked desktop UIA calls it a pane, so its
    # text is set by message
    Add-Type 'using System; using System.Runtime.InteropServices; public static class Txt { [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l); }'
    $edit = Find-Named $p.Id "$env:LOCALAPPDATA\Programs\Rookparcel" 20
    if (-not $edit) { Log "FAIL no folder box`n$(Window-Text $p.Id)"; Shot 'wizard-nofolder'; exit 1 }
    [Txt]::SendMessage([IntPtr]$edit.Current.NativeWindowHandle, 0x000C, [IntPtr]::Zero, $target) | Out-Null
    Start-Sleep 1
    Shot 'wizard-1-directory'
    Log "folder box now: $((Window-Text $p.Id) -match [regex]::Escape($target))"
    Next $p.Id 'Install' | Out-Null
    $ok = Find-Named $p.Id 'OK' 15
    Shot 'wizard-2-refused'
    Log "after Next:`n$(Window-Text $p.Id)"
    if ($ok) { Press $ok 1 | Out-Null; Start-Sleep 1 }
    Log "back on the page: $([bool]((Window-Text $p.Id) -match 'Destination Folder'))"
    Log "installed into it: $(Test-Path "$target\rookparcel.exe")"
    $c = Find-Named $p.Id 'Cancel' 10
    if ($c) { Press $c 2 | Out-Null; Start-Sleep 1 }
    $y = Find-Named $p.Id 'Yes' 10
    if ($y) { Press $y 6 | Out-Null }
    Start-Sleep 2
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $target
    exit 0
}

# uninstall: the settings of the default profile and of a profile, then "Remove settings" ticked
$s = "$env:APPDATA\Rookparcel"
New-Item -ItemType Directory -Force "$s\profiles\work\state" | Out-Null
Set-Content -Encoding ascii "$s\rookparcel.ini" 'x=1'
Set-Content -Encoding ascii "$s\rookparcel.lock" ''
Set-Content -Encoding ascii "$s\profiles\work\rookparcel.ini" 'x=1'
Log "before: $((Get-ChildItem -Recurse $s -Name) -join ', ')"
Start-Process "$env:LOCALAPPDATA\Programs\Rookparcel\uninstall.exe"
$u = 0; for ($i = 0; $i -lt 60 -and -not $u; $i++) { $q = Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -eq 'Un' -or $_.Name -like 'Un_*' -or $_.Name -eq 'Au_' } | Select-Object -First 1; if ($q) { $u = $q.Id } else { Start-Sleep -Milliseconds 500 } }
Log "uninstaller pid $u"
Next $u | Out-Null                        # Confirm
Start-Sleep 1
# the components page: tick "Remove settings" (the tree's second item) by message, as UIA sees
# only panes on a locked desktop: select it, then Space, which NSIS's tree toggles on
Add-Type @'
using System; using System.Text; using System.Runtime.InteropServices;
public static class Tree {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static IntPtr Find(IntPtr top, string cls) {
        IntPtr r = IntPtr.Zero;
        EnumChildWindows(top, (h, l) => { var s = new StringBuilder(64); GetClassName(h, s, 64); if (s.ToString() == cls) { r = h; return false; } return true; }, IntPtr.Zero);
        return r;
    }
}
'@
$top = [IntPtr]((Windows-Of $u) | Select-Object -First 1).Current.NativeWindowHandle
$tree = [IntPtr]::Zero
for ($i = 0; $i -lt 20 -and $tree -eq [IntPtr]::Zero; $i++) { $tree = [Tree]::Find($top, 'SysTreeView32'); Start-Sleep -Milliseconds 500 }
$first = [Tree]::SendMessage($tree, 0x110A, [IntPtr]0, [IntPtr]::Zero)
$second = [Tree]::SendMessage($tree, 0x110A, [IntPtr]1, $first)
$state = { ([int64][Tree]::SendMessage($tree, 0x1127, $second, [IntPtr]0xF000)) -shr 12 }
Log "tree $tree, second item state image $(& $state)"
[Tree]::SendMessage($tree, 0x110B, [IntPtr]9, $second) | Out-Null
[Tok]::PostMessage($tree, 0x0100, [IntPtr]0x20, [IntPtr]0x00390001) | Out-Null; Start-Sleep -Milliseconds 300
[Tok]::PostMessage($tree, 0x0101, [IntPtr]0x20, [IntPtr]0xC0390001) | Out-Null
Start-Sleep 1
Log "after Space: state image $(& $state)"
Shot 'uninstall-1-components'
Next $u 'Uninstall' | Out-Null
$close = Find-Named $u 'Close' 60
Shot 'uninstall-2-done'
if ($close) { Press $close 1 | Out-Null }
Start-Sleep 3
Log "after: settings folder $(Test-Path $s), profiles $(Test-Path "$s\profiles"), lock $(Test-Path "$s\rookparcel.lock"), program $(Test-Path "$env:LOCALAPPDATA\Programs\Rookparcel")"
exit 0
