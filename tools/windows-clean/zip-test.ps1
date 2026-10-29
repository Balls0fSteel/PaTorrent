# Clean-machine check of the Windows zip (run on the VM over SSH, Windows PowerShell 5.1).
param([string]$Version = '1.1.2', [string]$Root = 'C:\Users\x\patest')
$ErrorActionPreference = 'Stop'
$zip = Join-Path $root "patorrent-$Version-windows-x64.zip"
$dir = Join-Path $root 'zip'
$pass = 'zip-test-pass'

function Step($name) { Write-Output "== $name" }

function Login($port) {
    $s = New-Object Microsoft.PowerShell.Commands.WebRequestSession
    $r = Invoke-WebRequest -UseBasicParsing -WebSession $s -Method Post "http://127.0.0.1:$port/api/v2/auth/login" `
        -Body @{ username = 'admin'; password = $pass }
    $info = Invoke-WebRequest -UseBasicParsing -WebSession $s "http://127.0.0.1:$port/api/v2/torrents/info"
    $ver = Invoke-WebRequest -UseBasicParsing -WebSession $s "http://127.0.0.1:$port/api/v2/app/version"
    "login $($r.Content); torrents/info $($info.StatusCode) $($info.Content); app/version $($ver.Content)"
}

function WaitPort($port, $procId) {
    for ($i = 0; $i -lt 40; $i++) {
        if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) { return $true }
        if (-not (Get-Process -Id $procId -ErrorAction SilentlyContinue)) { return $false }
        Start-Sleep -Milliseconds 250
    }
    $false
}

Step 'unpack'
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
Expand-Archive -Path $zip -DestinationPath $dir
Get-ChildItem -Recurse $dir | ForEach-Object { "{0} {1}" -f $_.FullName.Substring($dir.Length + 1), $_.Length }
$gui = (Get-ChildItem -Recurse $dir -Filter patorrent.exe).FullName
$nox = (Get-ChildItem -Recurse $dir -Filter patorrent-nox.exe).FullName
foreach ($f in $gui, $nox) {
    $v = (Get-Item $f).VersionInfo
    "{0}: file {1}, product {2}, signed {3}" -f (Split-Path $f -Leaf), $v.FileVersion, $v.ProductVersion, (Get-AuthenticodeSignature $f).Status
}

Step 'nox --version'
& $nox --version

Step 'nox run with the Web UI'
$state = Join-Path $root 'noxstate'
Remove-Item -Recurse -Force $state -ErrorAction SilentlyContinue
$p = Start-Process -FilePath $nox -PassThru -WindowStyle Hidden `
    -RedirectStandardError (Join-Path $root 'nox.err') -RedirectStandardOutput (Join-Path $root 'nox.out') `
    -ArgumentList @('run', '--state-dir', $state, '--save', (Join-Path $root 'dl'), '--webui', '127.0.0.1:8090', '--webui-pass', $pass)
if (WaitPort 8090 $p.Id) { Login 8090 } else { "nox did not listen; exit $($p.ExitCode)"; Get-Content (Join-Path $root 'nox.err') -Tail 20 }
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue

Step 'GUI, portable, with its Web UI'
# portable mode: patorrent.ini beside the exe; the login is stored hashed, as Preferences does
$salt = New-Object byte[] 16
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($salt)
$kdf = New-Object Security.Cryptography.Rfc2898DeriveBytes($pass, $salt, 100000, [Security.Cryptography.HashAlgorithmName]::SHA256)
$hex = { param($b) ($b | ForEach-Object { $_.ToString('x2') }) -join '' }
$hash = 'pbkdf2-sha256:100000:' + (& $hex $salt) + ':' + (& $hex $kdf.GetBytes(32))
$guiDir = Split-Path $gui
@('webui.enabled=1', 'webui.username=admin', "webui.password=$hash", 'webui.listen=127.0.0.1:8091', 'webui.https=0') |
    Set-Content -Encoding ascii (Join-Path $guiDir 'patorrent.ini')
$g = Start-Process -FilePath $gui -WorkingDirectory $guiDir -PassThru
if (WaitPort 8091 $g.Id) { Login 8091 } else { "the GUI did not listen; running: $([bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)) exit $($g.ExitCode)" }
Start-Sleep 2
"GUI still running after start: " + [bool](Get-Process -Id $g.Id -ErrorAction SilentlyContinue)
"portable files: " + ((Get-ChildItem $guiDir -Name) -join ', ')
"appdata untouched: " + (-not (Test-Path "$env:APPDATA\PaTorrent"))
Stop-Process -Id $g.Id -Force -ErrorAction SilentlyContinue

Step 'Defender'
$t = Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object { $_.Resources -match 'patorrent' }
if ($t) { $t | Format-List } else { 'no detections' }
