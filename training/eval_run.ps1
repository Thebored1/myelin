# Eval: run stock vs tuned in llama-server, score on the held-out set, then stop them.
$ErrorActionPreference = "Continue"
$HERE = "C:\Users\Paper\StudioProjects\myelin\training"
Set-Location $HERE
$srv = "C:\Users\Paper\AppData\Local\Microsoft\WinGet\Packages\ggml.llamacpp_Microsoft.Winget.Source_8wekyb3d8bbwe\llama-server.exe"
$base = "C:\Users\Paper\Downloads\granite-4.0-h-1b-Q4_K_M.gguf"
# arg can be a tag (out\<tag>-Q4_K_M.gguf) or a full .gguf path (e.g. a different base model)
$tuned = if ($args.Count -ge 1) {
    if (Test-Path $args[0]) { $args[0] } else { "$HERE\out\$($args[0])-Q4_K_M.gguf" }
} else { "$HERE\out\granite-myelin2-Q4_K_M.gguf" }
# arg2 = ngl for the TUNED server, arg3 = ngl for the BASE server (so they can split the GPU/CPU)
$ngl = if ($args.Count -ge 2) { $args[1] } else { 99 }
$baseNgl = if ($args.Count -ge 3) { $args[2] } else { $ngl }

$p1 = Start-Process $srv -ArgumentList "-m `"$base`" --host 127.0.0.1 --port 8120 --jinja -ngl $baseNgl -c 2048" -PassThru -WindowStyle Hidden
$p2 = Start-Process $srv -ArgumentList "-m `"$tuned`" --host 127.0.0.1 --port 8121 --jinja -ngl $ngl -c 2048" -PassThru -WindowStyle Hidden
Write-Output "base=$base (ngl=$baseNgl)`ntuned=$tuned (ngl=$ngl)"
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
