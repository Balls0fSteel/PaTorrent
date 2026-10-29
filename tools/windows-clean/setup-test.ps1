# Clean-machine check of the Windows setup (run on the VM over SSH, Windows PowerShell 5.1):
# $OldVersion for the user with shortcuts and associations, $Version over it (refused while the GUI runs),
# the silent uninstall, then $Version for all users and its uninstall.
param([string]$Version = '1.1.2', [string]$OldVersion = '1.1.0', [string]$Root = 'C:\Users\x\patest')
$old = Join-Path $root "patorrent-$OldVersion-windows-x64-setup.exe"
$new = Join-Path $root "patorrent-$Version-windows-x64-setup.exe"
$failed = 0
function Step($name) { Write-Output "== $name" }
function Check($ok, $what) {
    if ($ok) { Write-Output "ok   $what" } else { Write-Output "FAIL $what"; $script:failed++ }
}
function Run($exe, [string[]]$a) { (Start-Process $exe -ArgumentList $a -Wait -PassThru).ExitCode }
function Default($path) { $i = Get-Item $path -ErrorAction SilentlyContinue; if ($i) { $i.GetValue('') } }
function FileVer($f) { (Get-Item $f).VersionInfo.FileVersion }
function WaitGone($paths) {
    for ($i = 0; $i -lt 60 -and ($paths | Where-Object { Test-Path $_ }); $i++) { Start-Sleep 1 }
}

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
Write-Output "elevated: $admin"

$dir = "$env:LOCALAPPDATA\Programs\PaTorrent"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent'
$menu = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\PaTorrent.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'PaTorrent.lnk'
$classes = 'HKCU:\Software\Classes'
$settings = "$env:APPDATA\PaTorrent"

Step "$OldVersion for the current user"
Check ((Run $old '/S', '/CURRENTUSER', '/DESKTOPSHORTCUT', '/ASSOCIATE=torrent,magnet') -eq 0) 'setup exit 0'
Check ((FileVer "$dir\patorrent.exe") -eq $OldVersion) "default folder $dir, $OldVersion"
Check ((Get-ItemProperty $key).DisplayVersion -eq $OldVersion) "Settings > Apps entry $OldVersion"
Check ((Test-Path $menu) -and (Test-Path $desktop)) 'Start menu and desktop shortcuts'
Check ((Default "$classes\.torrent") -eq 'PaTorrent.torrent') '.torrent associated'
Check ((Default "$classes\magnet\shell\open\command") -like "*$dir\patorrent.exe*") 'magnet: associated'

# settings of the old version: what the GUI keeps in %APPDATA%\PaTorrent, plus a marker
New-Item -ItemType Directory -Force $settings | Out-Null
Add-Content -Encoding ascii "$settings\patorrent.ini" 'test.marker=kept'

Step "$Version over it while the GUI runs"
$g = Start-Process "$dir\patorrent.exe" -PassThru
Start-Sleep 3
Check ([bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)) "installed $OldVersion GUI starts"
$code = Run $new '/S'
Check ($code -eq 2) "silent setup refuses (Cancel) while it runs: exit $code"
Check ((FileVer "$dir\patorrent.exe") -eq $OldVersion) 'nothing replaced'
Stop-Process -Id $g.Id -Force
Start-Sleep 1

Step "$Version over it"
Check ((Run $new '/S') -eq 0) 'setup exit 0'
Check ((FileVer "$dir\patorrent.exe") -eq $Version -and (FileVer "$dir\patorrent-nox.exe") -eq $Version) "same folder, files $Version"
$entries = @(Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' | Where-Object { $_.PSChildName -like '*PaTorrent*' })
Check ($entries.Count -eq 1) "one Settings > Apps entry ($($entries.Count))"
$u = Get-ItemProperty $key
$u | Format-List DisplayName, DisplayVersion, Publisher, DisplayIcon, InstallLocation, EstimatedSize, QuietUninstallString
Check ($u.DisplayVersion -eq $Version) "entry says $Version"
Check ((Test-Path $menu) -and (Test-Path $desktop)) 'shortcuts kept'
Check ((Default "$classes\.torrent") -eq 'PaTorrent.torrent') 'associations kept'
Check ((Get-Content "$settings\patorrent.ini") -contains 'test.marker=kept') 'settings kept'
& "$dir\patorrent-nox.exe" --version
$g = Start-Process "$dir\patorrent.exe" -PassThru
Start-Sleep 3
Check ([bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)) "installed $Version GUI starts"
Stop-Process -Id $g.Id -Force
Start-Sleep 1

Step 'silent uninstall (as Settings > Apps)'
$q = $u.QuietUninstallString
$exe = ($q -split '"')[1]
Start-Process $exe -ArgumentList '/S' -Wait
WaitGone @($key, $dir)
Check (-not (Test-Path $dir)) 'folder gone'
Check (-not (Test-Path $key)) 'entry gone'
Check (-not (Test-Path $menu) -and -not (Test-Path $desktop)) 'shortcuts gone'
Check (-not (Default "$classes\.torrent") -and -not (Test-Path "$classes\magnet")) 'associations gone'
Check (Test-Path "$settings\patorrent.ini") 'settings stay (not asked to remove them)'
Remove-Item -Recurse -Force $settings

Step "$Version for all users"
$pdir = "$env:ProgramFiles\PaTorrent"
$pkey = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PaTorrent'
$pmenu = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\PaTorrent.lnk"
Check ((Run $new '/S', '/ALLUSERS') -eq 0) 'setup exit 0'
Check ((FileVer "$pdir\patorrent.exe") -eq $Version) "files in $pdir"
Check ((Get-ItemProperty $pkey -ErrorAction SilentlyContinue).DisplayVersion -eq $Version) 'machine-wide entry'
Check (Test-Path $pmenu) 'Start menu shortcut for all users'
Check (-not (Test-Path $key)) 'no per-user entry'
$exe = ((Get-ItemProperty $pkey).QuietUninstallString -split '"')[1]
Start-Process $exe -ArgumentList '/S' -Wait
WaitGone @($pkey, $pdir)
Check (-not (Test-Path $pdir) -and -not (Test-Path $pkey) -and -not (Test-Path $pmenu)) 'uninstalled: folder, entry, shortcut gone'

Write-Output "== failures: $failed"
