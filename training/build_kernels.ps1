# Compile causal-conv1d for the RTX 2050 (sm_86) from the local clone — no PyPI
# fetch (which once hung on a missing prebuilt wheel). Verbose, with markers.
# GUARDRAIL: always run this with a bounded tool timeout so a hang can't run forever.
$ErrorActionPreference = "Continue"
$HERE = "C:\Users\Paper\StudioProjects\myelin\training"
Set-Location $HERE

# MSVC env
$vsPath = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
cmd /c "`"$vsPath\VC\Auxiliary\Build\vcvars64.bat`" && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item "env:$($matches[1])" $matches[2] }
}
Write-Output "MSVC: $((Get-Command cl -ErrorAction SilentlyContinue).Source)"

# CUDA env (conda prefix layout)
$env:CUDA_HOME = "$HERE\cudaenv"; $env:CUDA_PATH = $env:CUDA_HOME
$env:PATH = "$env:CUDA_HOME\bin;$env:PATH"
Write-Output "nvcc: $((Get-Command nvcc -ErrorAction SilentlyContinue).Source)"

# Build settings: only the RTX 2050 arch; force local build; cap parallelism for RAM.
$env:TORCH_CUDA_ARCH_LIST = "8.6"
$env:CAUSAL_CONV1D_FORCE_BUILD = "TRUE"
$env:MAX_JOBS = "4"
$env:DISTUTILS_USE_SDK = "1"
# VS 18 (MSVC 14.50) is newer than CUDA 12.4 nvcc officially supports — override
# the host-compiler version gate (nvcc applies these flags to every invocation).
$env:NVCC_PREPEND_FLAGS = "-allow-unsupported-compiler"

$venv = "$HERE\.venv\Scripts\python.exe"
Set-Location "$HERE\causal-conv1d"
Write-Output "=== BUILD START ==="
& $venv setup.py bdist_wheel
$code = $LASTEXITCODE
Write-Output "=== BUILD EXIT $code ==="
if ($code -eq 0) {
    $whl = Get-ChildItem dist\*.whl | Select-Object -First 1
    Write-Output "wheel: $($whl.Name)"
    uv pip install --python $venv --no-deps --no-cache $whl.FullName
    & $venv -c "from causal_conv1d import causal_conv1d_fn, causal_conv1d_update; print('CAUSAL_CONV1D OK')"
}
Write-Output "=== DONE ==="
