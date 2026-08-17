use super::*;
use anyhow::{anyhow, bail, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
pub(super) fn resolve_candidates(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
    preference: &str,
) -> Result<Vec<BackendCandidate>> {
    if let Ok(path) = env::var("MYELIN_LLAMA_SERVER_PATH") {
        let exe = validate_existing_file(resolve_input_path(app_data_dir, &path), "llama-server")?;
        return Ok(vec![BackendCandidate {
            backend: GpuBackend::Custom,
            executable_path: exe,
            engine: "llama_cpp".into(),
        }]);
    }

    let mut candidates: Vec<BackendCandidate> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let push = |candidates: &mut Vec<BackendCandidate>,
                seen: &mut Vec<PathBuf>,
                backend: GpuBackend,
                exe: PathBuf,
                engine: &str| {
        if exe.is_file() && !seen.contains(&exe) {
            seen.push(exe.clone());
            candidates.push(BackendCandidate {
                backend,
                executable_path: exe,
                engine: engine.to_string(),
            });
        }
    };

    // Collect tiering roots in priority order.
    let roots = tiering_roots(app_data_dir, workspace_config);
    let configured_exe = workspace_config
        .executable_path
        .as_ref()
        .map(|raw| resolve_input_path(app_data_dir, raw));

    // An explicitly configured executable is an authoritative runtime choice.
    // Do not let the automatic bundled-backend scan replace a user-selected
    // fork (for example a CPU-only Maple build) with stock llama.cpp. The
    // launcher adds the executable's directory to the library search path, so
    // sibling shared libraries remain available without trying other engines.
    if let Some(exe) = configured_exe {
        let exe = validate_existing_file(exe, "configured llama-server")?;
        return Ok(vec![BackendCandidate {
            backend: GpuBackend::Custom,
            executable_path: exe,
            engine: "llama_cpp".into(),
        }]);
    }

    // Experimental Bee candidates are isolated under bin/bee and precede stock
    // only when explicitly selected. Stock remains the automatic fallback.
    if normalize_engine(workspace_config.inference_engine.as_deref()) == "beellama" {
        let bee_root = app_data_dir.join("bin").join("bee");
        for backend in desired_backends(preference) {
            if let Some(dir) = backend.dir_name() {
                push(
                    &mut candidates,
                    &mut seen,
                    backend,
                    bee_root.join(dir).join(executable_name()),
                    "beellama",
                );
            }
        }
    }

    // Stock GPU/CPU backend subfolders, per detected hardware, across all roots.
    for backend in desired_backends(preference) {
        if let Some(dir) = backend.dir_name() {
            for root in &roots {
                push(
                    &mut candidates,
                    &mut seen,
                    backend,
                    root.join(dir).join(executable_name()),
                    "llama_cpp",
                );
            }
        }
    }

    // Flat fallbacks: the configured binary, each root's flat binary, then
    // PATH. These are `Custom` (unknown) rather than `Cpu` — a flat extraction
    // may itself be a GPU build, so it keeps the configured gpu_layers and the
    // real backend is detected from the startup log.
    for root in &roots {
        push(
            &mut candidates,
            &mut seen,
            GpuBackend::Custom,
            root.join(executable_name()),
            "llama_cpp",
        );
    }
    if let Some(path) = find_on_path(executable_name()) {
        push(
            &mut candidates,
            &mut seen,
            GpuBackend::Custom,
            path,
            "llama_cpp",
        );
    }

    if candidates.is_empty() {
        bail!(
            "llama-server not found. Set MYELIN_LLAMA_SERVER_PATH, add executablePath in app settings, install a backend under {}, or put {} on PATH.",
            app_data_dir.join("bin").display(),
            executable_name()
        );
    }

    Ok(candidates)
}

pub(super) fn resolve_model_path(
    app_data_dir: &Path,
    workspace_config: &WorkspaceLlamaConfig,
) -> Result<PathBuf> {
    if let Ok(path) = env::var("MYELIN_LLAMA_MODEL_PATH") {
        return validate_existing_model(resolve_input_path(app_data_dir, &path));
    }

    if let Some(path) = &workspace_config.model_path {
        return validate_existing_model(resolve_input_path(app_data_dir, path));
    }

    let models = discover_gguf_models(app_data_dir)?;
    match models.as_slice() {
        [] => bail!(
            "no .gguf model found. Set MYELIN_LLAMA_MODEL_PATH, choose a model in Settings, or place a single .gguf file in the app data directory."
        ),
        [model] => Ok(model.clone()),
        _ => {
            let preferred = models
                .iter()
                .find(|path| {
                    path.components().any(|component| {
                        component.as_os_str().to_string_lossy().eq_ignore_ascii_case("models")
                    })
                })
                .cloned();

            preferred.ok_or_else(|| {
                anyhow!(
                    "multiple .gguf models found. Choose one explicitly in Settings."
                )
            })
        }
    }
}

fn discover_gguf_models(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut models = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                ".git" | "node_modules" | "target" | "build" | "dist"
            )
        })
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false)
        {
            models.push(entry.path().to_path_buf());
        }
    }

    models.sort();
    Ok(models)
}

pub(super) fn validate_existing_file(path: PathBuf, label: &str) -> Result<PathBuf> {
    if path.is_file() {
        Ok(path)
    } else {
        bail!("{} path does not exist: {}", label, path.display())
    }
}

fn validate_existing_model(path: PathBuf) -> Result<PathBuf> {
    if !path.is_file() {
        bail!("model path does not exist: {}", path.display());
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("gguf"))
        .unwrap_or(false)
    {
        bail!("model must be a .gguf file: {}", path.display());
    }
    Ok(path)
}

pub(super) fn resolve_input_path(workspace: &Path, raw: &str) -> PathBuf {
    let input = PathBuf::from(raw);
    if input.is_absolute() {
        input
    } else {
        workspace.join(input)
    }
}

pub(super) fn executable_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// On Linux/macOS the dynamic loader does not search the executable's own
/// directory, so a relocatable llama-server can't find its sibling
/// `libggml*.so` / `libllama.so` and fails to launch (which surfaces as an
/// unreachable server / rig completion error). Prepend the binary's directory
/// to the child's library search path. No-op on Windows, where DLLs load from
/// the executable's directory automatically.
pub(super) fn apply_library_path(command: &mut Command, executable: &Path) {
    #[cfg(not(target_os = "windows"))]
    if let Some(dir) = executable.parent() {
        let var = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = env::var_os(var) {
            paths.extend(env::split_paths(&existing));
        }
        if let Ok(joined) = env::join_paths(paths) {
            command.env(var, joined);
        }
    }
    #[cfg(target_os = "windows")]
    let _ = (command, executable);
}

/// llama.cpp's Linux/macOS archives ship versioned libraries
/// (`libllama.so.0.0.9585`) alongside soname symlinks (`libllama.so.0`) that the
/// binary actually needs. Copying only regular files drops those symlinks, so
/// recreate them: for every `lib*.so.<major>...` real file, ensure a
/// `lib*.so.<major>` symlink exists. Idempotent and best-effort. No-op on
/// Windows (DLLs aren't versioned this way).
pub(super) fn ensure_sonames(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(idx) = name.find(".so.") else {
                continue;
            };
            let major: String = name[idx + 4..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if major.is_empty() {
                continue;
            }
            let soname = format!("{}.so.{}", &name[..idx], major);
            if soname == name {
                continue; // already the bare soname
            }
            let link = dir.join(&soname);
            if !link.exists() {
                let _ = symlink(&name, &link); // relative target, best-effort
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn find_on_path(binary_name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for directory in env::split_paths(&path_var) {
        let candidate = directory.join(binary_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
