# Dot-source before running training/eval/memcheck: sets up MSVC + CUDA + Triton
# so the Granite-4.0-h Mamba-2 fast path (Triton scan + causal-conv1d) works.
$HERE = "C:\Users\Paper\StudioProjects\myelin\training"
$vsPath = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -property installationPath
cmd /c "`"$vsPath\VC\Auxiliary\Build\vcvars64.bat`" && set" | ForEach-Object {
    if ($_ -match '^([^=]+)=(.*)$') { Set-Item "env:$($matches[1])" $matches[2] }
}
$env:CUDA_HOME = "$HERE\cudaenv"; $env:CUDA_PATH = $env:CUDA_HOME
$env:PATH = "$env:CUDA_HOME\bin;$env:PATH"
$env:KMP_DUPLICATE_LIB_OK = "TRUE"
$env:PYTORCH_CUDA_ALLOC_CONF = "expandable_segments:True"
$env:NVCC_PREPEND_FLAGS = "-allow-unsupported-compiler"   # VS18 host compiler override
