# Eval: run stock vs tuned in llama-server, score on the held-out set, then stop them.
$ErrorActionPreference = "Continue"
$HERE = "C:\Users\Paper\StudioProjects\myelin\training"
Set-Location $HERE
$srv = "C:\Users\Paper\AppData\Local\Microsoft\WinGet\Packages\ggml.llamacpp_Microsoft.Winget.Source_8wekyb3d8bbwe\llama-server.exe"
$base = "C:\Users\Paper\Downloads\granite-4.0-h-1b-Q4_K_M.gguf"
$tuned = if ($args.Count -ge 1) { "$HERE\out\$($args[0])-Q4_K_M.gguf" } else { "$HERE\out\granite-myelin2-Q4_K_M.gguf" }

$p1 = Start-Process $srv -ArgumentList "-m `"$base`" --host 127.0.0.1 --port 8120 --jinja -ngl 99 -c 2048" -PassThru -WindowStyle Hidden
$p2 = Start-Process $srv -ArgumentList "-m `"$tuned`" --host 127.0.0.1 --port 8121 --jinja -ngl 99 -c 2048" -PassThru -WindowStyle Hidden
Write-Output "servers starting (pids $($p1.Id), $($p2.Id)) ..."

foreach ($port in 8120, 8121) {
    $ok = $false
    for ($i = 0; $i -lt 90; $i++) {
        try { Invoke-WebRequest "http://127.0.0.1:$port/health" -TimeoutSec 2 -UseBasicParsing | Out-Null; $ok = $true; break }
        catch { Start-Sleep -Seconds 2 }
    }
    Write-Output "port $port ready: $ok"
}

& "$HERE\.venv\Scripts\python.exe" eval.py --base-url http://127.0.0.1:8120 --tuned-url http://127.0.0.1:8121 --base-system-file "$HERE\app_preamble.txt"
Stop-Process -Id $p1.Id, $p2.Id -Force -ErrorAction SilentlyContinue
Write-Output "=== EVAL DONE ==="
