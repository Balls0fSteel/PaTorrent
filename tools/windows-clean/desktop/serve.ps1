# Serves a folder over http://localhost:<port>/ for the update hand test (Windows PowerShell 5.1).
param([string]$Dir = 'C:\Users\x\patest\upd', [int]$Port = 8765, [int]$Delay = 0,[string]$Log = 'C:\Users\x\patest\serve.log')
$l = New-Object System.Net.HttpListener
$l.Prefixes.Add("http://localhost:$Port/")
$l.Prefixes.Add("http://127.0.0.1:$Port/")
$l.Start()
Add-Content $Log "$(Get-Date -f s) listening"
while ($l.IsListening) {
    $c = $l.GetContext()
    $name = [Uri]::UnescapeDataString($c.Request.Url.AbsolutePath.TrimStart('/'))
    $path = Join-Path $Dir $name
    Add-Content $Log "$(Get-Date -f s) $($c.Request.HttpMethod) /$name"
    if ($name -eq 'quit') { $c.Response.Close(); break }
    if ($name -and (Test-Path -LiteralPath $path -PathType Leaf)) {
        $bytes = [IO.File]::ReadAllBytes($path)
        $c.Response.ContentLength64 = $bytes.Length
        # -Delay: 32 KiB every $Delay ms, so a download can be cancelled
        $step = if ($Delay) { 32768 } else { $bytes.Length }
        try {
            for ($o = 0; $o -lt $bytes.Length; $o += $step) {
                $c.Response.OutputStream.Write($bytes, $o, [Math]::Min($step, $bytes.Length - $o))
                if ($Delay) { Start-Sleep -Milliseconds $Delay }
            }
        } catch { Add-Content $Log "$(Get-Date -f s) aborted by the client at $o" }
    } else {
        $c.Response.StatusCode = 404
    }
    $c.Response.Close()
}
$l.Stop()
