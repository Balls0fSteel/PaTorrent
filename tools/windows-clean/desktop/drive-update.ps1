# D45 hand test in the console session: start the installed PaTorrent against a local update.txt,
# take the update box's "Download and install", answer "Close and install now?" Yes, and watch the
# setup replace it and start the new version. Mode: 'user' or 'all'. Cancel: press Cancel in the
# progress window instead and check %TEMP%\PaTorrent-update is left empty.
param([string]$Mode = 'user', [switch]$Cancel, [string]$Tag = 'update')
$LogFile = "C:\Users\x\patest\$Tag.log"; $ShotDir = 'C:\Users\x\patest\shots'
New-Item -ItemType Directory -Force $ShotDir | Out-Null
. C:\Users\x\patest\uia.ps1
$dir = if ($Mode -eq 'all') { "$env:ProgramFiles\PaTorrent" } else { "$env:LOCALAPPDATA\Programs\PaTorrent" }
$key = if ($Mode -eq 'all') { 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent' } else { 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent' }
$exe = "$dir\patorrent.exe"
Log "start: $Mode cancel=$Cancel; entry $((Get-ItemProperty $key).DisplayVersion), exe written $((Get-Item $exe).LastWriteTime.ToString('s'))"
$env:PATORRENT_UPDATE_URL = 'http://localhost:8765/update.txt'
$env:PATORRENT_UPDATE_FILES = 'http://localhost:8765/'
$p = Start-Process $exe -PassThru
Log "gui pid $($p.Id) elevated=$([Tok]::Elevated($p.Id))"
$b = Find-Named $p.Id 'Download and install' 40
if (-not $b) { Log "FAIL no update box`n$(Window-Text $p.Id)"; Shot "$Tag-nobox"; exit 1 }
Log "box:`n$(Window-Text $p.Id)"
Shot "$Tag-1-box"
Press $b 100 | Out-Null
if ($Cancel) {
    Start-Sleep 2
    $c = Find-Named $p.Id 'Cancel' 15
    Shot "$Tag-2-progress"
    Log "progress:`n$(Window-Text $p.Id)"
    Press $c 2 | Out-Null
    Start-Sleep 2
    $left = @(Get-ChildItem "$env:TEMP\PaTorrent-update" -ErrorAction SilentlyContinue)
    Log "after cancel: files in %TEMP%\PaTorrent-update: $($left.Count) $($left.Name -join ', '); gui alive $([bool](Get-Process -Id $p.Id -EA SilentlyContinue))"
    Shot "$Tag-3-cancelled"
    Stop-Process -Id $p.Id -Force
    exit 0
}
$y = Find-Named $p.Id 'Yes' 120
if (-not $y) { Log "FAIL no install question`n$(Window-Text $p.Id)"; Shot "$Tag-noq"; exit 1 }
Log "question:`n$(Window-Text $p.Id)"
Shot "$Tag-2-question"
Press $y 6 | Out-Null
$t = Get-Date
while ((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -and ((Get-Date) - $t).TotalSeconds -lt 60) { Start-Sleep -Milliseconds 250 }
Log "old gui exited after $([int]((Get-Date) - $t).TotalSeconds) s"
$new = $null
for ($i = 0; $i -lt 240 -and -not $new; $i++) {
    Start-Sleep -Milliseconds 500
    $new = Get-Process patorrent -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $p.Id -and $_.Path -eq $exe } | Select-Object -First 1
}
if (-not $new) { Log "FAIL no new PaTorrent within 2 min"; exit 1 }
Start-Sleep 3
Log "new gui pid $($new.Id) elevated=$([Tok]::Elevated($new.Id)) ; entry $((Get-ItemProperty $key).DisplayVersion), exe written $((Get-Item $exe).LastWriteTime.ToString('s'))"
Log "window:`n$(Window-Text $new.Id)"
Shot "$Tag-4-new"
$left = @(Get-ChildItem "$env:TEMP\PaTorrent-update" -ErrorAction SilentlyContinue)
Log "files in %TEMP%\PaTorrent-update: $($left.Count) $($left.Name -join ', ')"
Stop-Process -Id $new.Id -Force
Log 'done'
