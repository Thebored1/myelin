# After training: merge the LoRA into the base, convert to GGUF, quantize Q4_K_M.
# Run from training/ after out/adapter exists.
$ErrorActionPreference = "Continue"
$HERE = "C:\Users\Paper\StudioProjects\myelin\training"
Set-Location $HERE
. .\runenv.ps1
$venv = "$HERE\.venv\Scripts\python.exe"

$adapter = if ($args.Count -ge 1) { $args[0] } else { "out\adapter2" }
$tag = if ($args.Count -ge 2) { $args[1] } else { "granite-myelin2" }
Write-Output "=== 1/3 merge LoRA ($adapter) -> out\merged ==="
& $venv train_lora.py --merge --base base\granite-4.0-h-1b --out $adapter --merged out\merged

Write-Output "=== 2/3 convert HF -> GGUF (f16) ==="
# convert_hf_to_gguf needs the gguf module from the llama.cpp checkout.
$env:PYTHONPATH = "$HERE\llama.cpp\gguf-py"
& $venv llama.cpp\convert_hf_to_gguf.py out\merged --outfile "out\$tag-f16.gguf" --outtype f16

Write-Output "=== 3/3 quantize Q4_K_M ==="
$quant = "C:\Users\Paper\AppData\Local\Microsoft\WinGet\Packages\ggml.llamacpp_Microsoft.Winget.Source_8wekyb3d8bbwe\llama-quantize.exe"
& $quant "out\$tag-f16.gguf" "out\$tag-Q4_K_M.gguf" Q4_K_M

Write-Output "=== DONE ==="
Get-ChildItem out\*.gguf | Select-Object Name, @{n = 'MB'; e = { [math]::Round($_.Length / 1MB) } }
