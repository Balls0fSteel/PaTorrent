# Clean-machine check of the Windows setup (run on the VM over SSH, Windows PowerShell 5.1):
# $OldVersion for the user with shortcuts and associations, $Version over it (refused while the GUI runs),
# a profile's nox service holding the uninstall, the silent uninstall, a portable target refused, then
# $Version for all users and its uninstall. The SSH login is elevated (service install needs it).
param([string]$Version = '1.1.2', [string]$OldVersion = '1.1.0', [string]$Root = 'C:\Users\x\patest')
$old = Join-Path $root "rookparcel-$OldVersion-windows-x64-setup.exe"
$new = Join-Path $root "rookparcel-$Version-windows-x64-setup.exe"
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

$dir = "$env:LOCALAPPDATA\Programs\Rookparcel"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Rookparcel'
$menu = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Rookparcel.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Rookparcel.lnk'
$classes = 'HKCU:\Software\Classes'
$settings = "$env:APPDATA\Rookparcel"

Step "$OldVersion for the current user"
Check ((Run $old '/S', '/CURRENTUSER', '/DESKTOPSHORTCUT', '/ASSOCIATE=torrent,magnet') -eq 0) 'setup exit 0'
Check ((FileVer "$dir\rookparcel.exe") -eq $OldVersion) "default folder $dir, $OldVersion"
Check ((Get-ItemProperty $key).DisplayVersion -eq $OldVersion) "Settings > Apps entry $OldVersion"
Check ((Test-Path $menu) -and (Test-Path $desktop)) 'Start menu and desktop shortcuts'
Check ((Default "$classes\.torrent") -eq 'Rookparcel.torrent') '.torrent associated'
Check ((Default "$classes\magnet\shell\open\command") -like "*$dir\rookparcel.exe*") 'magnet: associated'

# settings of the old version: what the GUI keeps in %APPDATA%\Rookparcel, plus a marker
New-Item -ItemType Directory -Force $settings | Out-Null
Add-Content -Encoding ascii "$settings\rookparcel.ini" 'test.marker=kept'

Step "$Version over it while the GUI runs"
$g = Start-Process "$dir\rookparcel.exe" -PassThru
Start-Sleep 3
Check ([bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)) "installed $OldVersion GUI starts"
$code = Run $new '/S'
Check ($code -eq 2) "silent setup refuses (Cancel) while it runs: exit $code"
Check ((FileVer "$dir\rookparcel.exe") -eq $OldVersion) 'nothing replaced'
Stop-Process -Id $g.Id -Force
Start-Sleep 1

Step "$Version over it"
Check ((Run $new '/S') -eq 0) 'setup exit 0'
Check ((FileVer "$dir\rookparcel.exe") -eq $Version -and (FileVer "$dir\rookparcel-nox.exe") -eq $Version) "same folder, files $Version"
$entries = @(Get-ChildItem 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall' | Where-Object { $_.PSChildName -like '*Rookparcel*' })
Check ($entries.Count -eq 1) "one Settings > Apps entry ($($entries.Count))"
$u = Get-ItemProperty $key
$u | Format-List DisplayName, DisplayVersion, Publisher, DisplayIcon, InstallLocation, EstimatedSize, QuietUninstallString
Check ($u.DisplayVersion -eq $Version) "entry says $Version"
Check ((Test-Path $menu) -and (Test-Path $desktop)) 'shortcuts kept'
Check ((Default "$classes\.torrent") -eq 'Rookparcel.torrent') 'associations kept'
Check ((Get-Content "$settings\rookparcel.ini") -contains 'test.marker=kept') 'settings kept'
& "$dir\rookparcel-nox.exe" --version
$g = Start-Process "$dir\rookparcel.exe" -PassThru
Start-Sleep 3
Check ([bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)) "installed $Version GUI starts"
Stop-Process -Id $g.Id -Force
Start-Sleep 1

Step "a profile's nox service holds the uninstall (D42, 1.1.47 and later)"
$nox = "$dir\rookparcel-nox.exe"
& $nox service install --profile x --start manual --state-dir "$root\noxsvc-x" | Out-Null
Check ($LASTEXITCODE -eq 0) 'service install --profile x'
Check ((sc.exe query RookparcelNox_x | Out-String) -match 'RookparcelNox_x') 'RookparcelNox_x registered'
& $nox service status --all --in $dir | Out-Null
Check ($LASTEXITCODE -eq 0) 'service status --all --in finds it'
$q = $u.QuietUninstallString
$exe = ($q -split '"')[1]
Start-Process $exe -ArgumentList '/S' -Wait
Start-Sleep 5
Check (Test-Path "$dir\rookparcel-nox.exe") 'the silent uninstall refuses (Cancel) while the service is registered'
& $nox service uninstall --all | Out-Null
Check ($LASTEXITCODE -eq 0 -and -not ((sc.exe query RookparcelNox_x | Out-String) -match 'RUNNING|STOPPED')) 'service uninstall --all'
Remove-Item -Recurse -Force "$root\noxsvc-x" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$settings\profiles\work" | Out-Null

Step 'silent uninstall (as Settings > Apps)'
Start-Process $exe -ArgumentList '/S' -Wait
WaitGone @($key, $dir)
Check (-not (Test-Path $dir)) 'folder gone'
Check (-not (Test-Path $key)) 'entry gone'
Check (-not (Test-Path $menu) -and -not (Test-Path $desktop)) 'shortcuts gone'
Check (-not (Default "$classes\.torrent") -and -not (Test-Path "$classes\magnet")) 'associations gone'
Check (Test-Path "$settings\rookparcel.ini") 'settings stay (not asked to remove them)'
# "Remove settings" (unticked by default) also removes profiles\: a check by hand in the wizard
Check (Test-Path "$settings\profiles\work") 'profiles stay too'
Remove-Item -Recurse -Force $settings

Step 'the setup refuses a portable folder (D42, 1.1.47 and later)'
$pf = Join-Path $root 'portable-target'
New-Item -ItemType Directory -Force $pf | Out-Null
Set-Content -Encoding ascii "$pf\rookparcel.ini" ''
$code = Run $new '/S', '/CURRENTUSER', "/D=$pf"
Check ($code -ne 0 -and -not (Test-Path "$pf\rookparcel.exe")) "refused: exit $code, nothing installed"
Check (-not (Test-Path $key)) 'no Settings > Apps entry'
Remove-Item -Recurse -Force $pf

Step "$Version for all users"
$pdir = "$env:ProgramFiles\Rookparcel"
$pkey = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Rookparcel'
$pmenu = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Rookparcel.lnk"
Check ((Run $new '/S', '/ALLUSERS') -eq 0) 'setup exit 0'
Check ((FileVer "$pdir\rookparcel.exe") -eq $Version) "files in $pdir"
Check ((Get-ItemProperty $pkey -ErrorAction SilentlyContinue).DisplayVersion -eq $Version) 'machine-wide entry'
Check (Test-Path $pmenu) 'Start menu shortcut for all users'
Check (-not (Test-Path $key)) 'no per-user entry'
$exe = ((Get-ItemProperty $pkey).QuietUninstallString -split '"')[1]
Start-Process $exe -ArgumentList '/S' -Wait
WaitGone @($pkey, $pdir)
Check (-not (Test-Path $pdir) -and -not (Test-Path $pkey) -and -not (Test-Path $pmenu)) 'uninstalled: folder, entry, shortcut gone'

Write-Output "== failures: $failed"
