# D47 closing-window test in the console session. The two seeding torrents (12 GB and 4 GB) are
# moved to the other drive (C:\Users\x\patest\data <-> T:\dest) through the Web UI's qBittorrent
# API; File > Exit follows 2 s into the move. The closing window is read twice, then Quit Now > Yes
# (or, with -NoQuit, the moves are left to finish). Afterwards: the old folder must be whole and
# the destination clean (Quit Now), and a restart must seed from wherever the files are.
param([string]$Tag = 'closing', [switch]$NoQuit, [int]$Wait = 4)
$LogFile = "C:\Users\x\patest\$Tag.log"; $ShotDir = 'C:\Users\x\patest\shots'
. C:\Users\x\patest\uia.ps1
$root = 'C:\Users\x\patest'
$exe = "$env:LOCALAPPDATA\Programs\PaTorrent\patorrent.exe"
$api = 'http://127.0.0.1:8080/api/v2'
# PowerShell 5.1's ConvertFrom-Json gives a JSON array as one object: unroll it
function Info { (Invoke-WebRequest -UseBasicParsing "$api/torrents/info").Content | ConvertFrom-Json | ForEach-Object { $_ } }
function Start-Gui {
    $p = Get-Process patorrent -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $p) { $p = Start-Process $exe -PassThru }
    for ($i = 0; $i -lt 60; $i++) {
        try { Invoke-WebRequest -UseBasicParsing "$api/app/version" | Out-Null; break } catch { Start-Sleep 1 }
    }
    $p
}
function Wait-Seeding($n, $secs) {
    for ($i = 0; $i -lt $secs; $i++) {
        $t = @(Info)
        if ($t.Count -ge $n -and -not ($t | Where-Object { $_.progress -lt 1 -or $_.state -match 'checking|moving' })) { return $t }
        Start-Sleep 1
    }
    @(Info)
}
function Show($t) { ($t | ForEach-Object { "$($_.name) $($_.state) $([math]::Round($_.progress * 100))% in $($_.save_path)" }) -join ' | ' }
function Tree($dir) {
    if (-not (Test-Path $dir)) { return "$dir absent" }
    $f = @(Get-ChildItem $dir -Recurse -File | ForEach-Object { "$($_.FullName) $($_.Length)" })
    if ($f) { $f -join '; ' } else { "$dir empty" }
}
function Exit-Gui($procId) {
    $h = Main-Hwnd $procId
    $ok = [Tok]::PostMessage($h, 0x111, [IntPtr]103, [IntPtr]::Zero)
    Log ("File > Exit posted to {0:X}: {1}" -f $h.ToInt64(), $ok)
}

$p = Start-Gui
Log "gui pid $($p.Id)"
if (@(Info).Count -lt 2) {
    & $exe "$root\big1.torrent"; Start-Sleep 1; & $exe "$root\big2.torrent"
}
$t = Wait-Seeding 2 300
Log "seeding: $(Show $t)"
$from = $t[0].save_path.TrimEnd('\')
$to = if ($from -like 'T:*') { "$root\data" } else { 'T:\dest' }
New-Item -ItemType Directory -Force $to | Out-Null
Log "before: source $(Tree $from); destination $(Tree $to)"
$r = Invoke-WebRequest -UseBasicParsing -Method Post "$api/torrents/setLocation" -Body @{ hashes = (($t | ForEach-Object { $_.hash }) -join '|'); location = $to }
Log "setLocation $to : HTTP $($r.StatusCode)"
Start-Sleep 2
Log "2 s in: $(Show (Info))"
Exit-Gui $p.Id
$exitAt = Get-Date
# the closing window and the question are found by handle: they are not under UIA's root
function Hwnd-Of($line) { [IntPtr][Convert]::ToInt64(($line -split ' ')[0], 16) }
function Text-Of([IntPtr]$h) {
    $e = $A::FromHandle($h)
    $out = @("[$($e.Current.ClassName)] $($e.Current.Name)")
    foreach ($c in $e.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
        if ($c.Current.Name) { $out += "  $($c.Current.ControlType.ProgrammaticName): $($c.Current.Name)" }
    }
    $out -join "`n"
}
function Wait-Dialog($title, $except, $secs) {
    for ($i = 0; $i -lt $secs * 4 -and (Get-Process -Id $p.Id -EA SilentlyContinue); $i++) {
        $w = [Wins]::Of([uint32]$p.Id) | Where-Object { $_ -match "visible True \[#32770\] $title" -and $_ -notlike "$except*" } | Select-Object -First 1
        if ($w) { return $w }
        Start-Sleep -Milliseconds 250
    }
    $null
}
$cw = Wait-Dialog 'PaTorrent is closing' 'none' 20
if (-not $cw) { Log "no closing window; alive $([bool](Get-Process -Id $p.Id -EA SilentlyContinue))" }
else {
    $ch = Hwnd-Of $cw
    Log ("closing window after {0:N1} s: $cw`n$(Text-Of $ch)" -f ((Get-Date) - $exitAt).TotalSeconds)
    Start-Sleep $Wait
    Log "4 s later:`n$(Text-Of $ch)"
    if (-not $NoQuit) {
        [Tok]::PostMessage($ch, 0x111, [IntPtr]104, [IntPtr]::Zero) | Out-Null
        $qw = Wait-Dialog 'PaTorrent$' ($cw -split ' ')[0] 10
        if (-not $qw) { Log 'no Quit Now question' }
        else {
            $qh = Hwnd-Of $qw
            Log "question: $qw`n$(Text-Of $qh)"
            $quitAt = Get-Date
            [Tok]::PostMessage($qh, 0x111, [IntPtr]6, [IntPtr]::Zero) | Out-Null
            Start-Sleep 1
            if (Get-Process -Id $p.Id -EA SilentlyContinue) { try { Log "after Yes:`n$(Text-Of $ch)" } catch { Log 'after Yes: closing window gone' } }
        }
    }
}
$t0 = Get-Date
while ((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -and ((Get-Date) - $t0).TotalSeconds -lt 600) { Start-Sleep -Milliseconds 200 }
$done = Get-Date
Log "exited: $(-not (Get-Process -Id $p.Id -EA SilentlyContinue)), $([math]::Round(($done - $exitAt).TotalSeconds, 1)) s after Exit$(if ($quitAt) { ", $([math]::Round(($done - $quitAt).TotalSeconds, 1)) s after Yes" })"
Log "source: $(Tree $from)"
Log "destination: $(Tree $to)"
$p = Start-Gui
$t = Wait-Seeding 2 300
Log "after restart: $(Show $t)"
Exit-Gui $p.Id
$t0 = Get-Date
while ((Get-Process -Id $p.Id -ErrorAction SilentlyContinue) -and ((Get-Date) - $t0).TotalSeconds -lt 60) { Start-Sleep -Milliseconds 200 }
Log "clean exit took $([math]::Round(((Get-Date) - $t0).TotalSeconds, 1)) s; alive $([bool](Get-Process -Id $p.Id -EA SilentlyContinue))"
Log 'done'
