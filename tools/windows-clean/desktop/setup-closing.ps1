# D47 closing-window test, elevated part (over SSH): a 24 GB virtual disk as T:, two torrents of
# zero-filled data on C:, and the installed GUI's settings (no Add dialog, the Web UI on
# 127.0.0.1:8080 without a login from this machine). -Down removes the disk and the data.
param([switch]$Down)
$root = 'C:\Users\x\patest'
$vhd = "$root\movetest.vhdx"
$dp = "$root\dp.txt"
if ($Down) {
    Set-Content -Encoding ascii $dp "select vdisk file=`"$vhd`"`r`ndetach vdisk"
    diskpart /s $dp | Out-Null
    Remove-Item -Force $vhd, $dp -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force "$root\data", "$root\*.torrent" -ErrorAction SilentlyContinue
    "down: T: $(Test-Path T:\), vhdx $(Test-Path $vhd)"
    return
}
if (-not (Test-Path T:\)) {
    Set-Content -Encoding ascii $dp "create vdisk file=`"$vhd`" maximum=24576 type=expandable`r`nattach vdisk`r`ncreate partition primary`r`nformat fs=ntfs quick label=MOVETEST`r`nassign letter=T"
    diskpart /s $dp | Select-Object -Last 3
}
foreach ($t in @(@{n = 'big1'; gb = 12}, @{n = 'big2'; gb = 4})) {
    $d = "$root\data\$($t.n)"
    New-Item -ItemType Directory -Force $d | Out-Null
    if (-not (Test-Path "$d\$($t.n).bin")) { fsutil file createnew "$d\$($t.n).bin" ([int64]$t.gb * 1GB) | Out-Null }
    if (-not (Test-Path "$root\$($t.n).torrent")) {
        & "$env:LOCALAPPDATA\Programs\Rookparcel\rookparcel-nox.exe" create $d -o "$root\$($t.n).torrent" --v1 --piece-size 4096 | Select-Object -Last 1
    }
}
$ini = "$env:APPDATA\Rookparcel\rookparcel.ini"
$keep = @(Get-Content $ini -ErrorAction SilentlyContinue | Where-Object { $_ -notmatch '^(ui\.show_add_dialog|dir\.download|webui\.|update\.seen)' })
$hash = (Get-Content "$root\hash.txt" -Raw).Trim()
$keep + @('ui.show_add_dialog=0', "dir.download=$root\data", 'webui.enabled=1', 'webui.listen=127.0.0.1:8080',
    "webui.password=$hash", 'webui.bypass_auth_subnets=127.0.0.1/32') | Set-Content -Encoding UTF8 $ini
Get-ChildItem "$root\data" -Recurse -File | Select-Object FullName, Length
Get-PSDrive T | Select-Object Name, Free
