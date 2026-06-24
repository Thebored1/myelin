"""Patch a fresh causal-conv1d v1.4.0 clone so it compiles on Windows (VS18 +
conda CUDA 12.4) for the RTX 2050. Idempotent. Called by build_kernels.ps1.

Four fixes, all needed:
  1. setup.py: build only sm_86 (the 2050) instead of the hardcoded 53/62/.../90.
  2. setup.py: add /FIiso646.h (and/or/not) + /Zc:preprocessor to the MSVC flags.
  3. *.cu/*.h: '#pragma unroll' -> '_Pragma("unroll")' (a '#' inside a macro arg is illegal).
  4. *.cu/*.h: resolve '#ifndef/#ifdef USE_ROCM' blocks (also '#' inside a macro arg).
"""
import re, glob, sys, pathlib

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "causal-conv1d")

# 1 + 2: setup.py
sp = root / "setup.py"
s = sp.read_text(encoding="utf-8")
s = re.sub(
    r'cc_flag\.append\("-gencode"\)\s*\n\s*cc_flag\.append\("arch=compute_53.*?arch=compute_90,code=sm_90"\)',
    '        cc_flag.append("-gencode")\n        cc_flag.append("arch=compute_86,code=sm_86")',
    s, flags=re.S)
if "/FIiso646.h" not in s:
    s = s.replace('"cxx": ["-O3"],', '"cxx": ["-O3", "/FIiso646.h", "/Zc:preprocessor"],')
    s = s.replace('"-lineinfo",',
                  '"-lineinfo",\n                    "-Xcompiler", "/FIiso646.h",\n                    "-Xcompiler", "/Zc:preprocessor",')
sp.write_text(s, encoding="utf-8")
print("patched setup.py (arch sm_86 + MSVC flags)")


def strip_rocm(text):
    out, state = [], None
    for l in text.split("\n"):
        t = l.strip()
        if state is None:
            if t == "#ifndef USE_ROCM": state = "keep"
            elif t == "#ifdef USE_ROCM": state = "drop"
            else: out.append(l)
        else:
            if t == "#else": state = "drop" if state == "keep" else "keep"
            elif t == "#endif": state = None
            elif state == "keep": out.append(l)
    return "\n".join(out)


def fix_pragma(m):
    n = (m.group(1) or "").strip()
    return '_Pragma("unroll %s")' % n if n else '_Pragma("unroll")'


# 3 + 4: source files
for f in glob.glob(str(root / "csrc" / "*.cu")) + glob.glob(str(root / "csrc" / "*.h")) \
        + glob.glob(str(root / "csrc" / "*.cuh")):
    p = pathlib.Path(f)
    txt = p.read_text(encoding="utf-8")
    txt = re.sub(r'#pragma\s+unroll(\s+\d+)?', fix_pragma, txt)
    if "USE_ROCM" in txt:
        txt = strip_rocm(txt)
    p.write_text(txt, encoding="utf-8")
print("patched csrc (pragma + USE_ROCM)")
