# Runs a script in user x's console session (from the elevated SSH login) through a one-off
# scheduled task, waits for it unless -NoWait, and removes the task. -High runs it elevated.
param([Parameter(Mandatory)][string]$Script, [string]$ScriptArgs = '', [string]$Name = 'patest', [switch]$High, [switch]$NoWait, [int]$Timeout = 600)
$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$Script`" $ScriptArgs"
$level = if ($High) { 'Highest' } else { 'Limited' }
$pr = New-ScheduledTaskPrincipal -UserId "$env:COMPUTERNAME\x" -LogonType Interactive -RunLevel $level
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries
Unregister-ScheduledTask -TaskName $Name -Confirm:$false -ErrorAction SilentlyContinue
Register-ScheduledTask -TaskName $Name -Action $a -Principal $pr -Settings $s | Out-Null
Start-ScheduledTask -TaskName $Name
if ($NoWait) { return }
Start-Sleep 2
$t = Get-Date
while ((Get-ScheduledTask -TaskName $Name).State -eq 'Running' -and ((Get-Date) - $t).TotalSeconds -lt $Timeout) { Start-Sleep 1 }
"task $Name result: $((Get-ScheduledTaskInfo -TaskName $Name).LastTaskResult)"
Unregister-ScheduledTask -TaskName $Name -Confirm:$false
