use super::*;
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
pub fn assets_for_backend(backend: &str) -> Vec<String> {
    let tag = LLAMA_RELEASE_TAG;
    if cfg!(target_os = "windows") {
        match backend {
            // CUDA needs the runtime DLLs (cudart) alongside the binaries.
            "cuda" => vec![
                "cudart-llama-bin-win-cuda-12.4-x64.zip".to_string(),
                format!("llama-{tag}-bin-win-cuda-12.4-x64.zip"),
            ],
            "vulkan" => vec![format!("llama-{tag}-bin-win-vulkan-x64.zip")],
            "cpu" => vec![format!("llama-{tag}-bin-win-cpu-x64.zip")],
            _ => vec![],
        }
    } else if cfg!(target_os = "linux") {
        match backend {
            "vulkan" => vec![format!("llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz")],
            "cpu" => vec![format!("llama-{tag}-bin-ubuntu-x64.tar.gz")],
            // No prebuilt CUDA tarball for Linux in this release; use Vulkan.
            _ => vec![],
        }
    } else if cfg!(target_os = "macos") {
        match backend {
            "metal" if cfg!(target_arch = "aarch64") => {
                vec![format!("llama-{tag}-bin-macos-arm64.tar.gz")]
            }
            "metal" if cfg!(target_arch = "x86_64") => {
                vec![format!("llama-{tag}-bin-macos-x64.tar.gz")]
            }
            _ => vec![],
        }
    } else {
        vec![]
    }
}

/// Backends that can be downloaded on demand for this OS.
pub fn downloadable_backends() -> Vec<String> {
    ["cuda", "vulkan", "metal", "cpu"]
        .iter()
        .filter(|b| !assets_for_backend(b).is_empty())
        .map(|b| b.to_string())
        .collect()
}

pub fn bee_assets_for_backend(backend: &str) -> Vec<String> {
    let prefix = format!("beellama-{BEELLAMA_RELEASE_TAG}-");
    if cfg!(target_os = "windows") {
        match backend {
            "cuda" => vec![
                format!("{prefix}cudart-win-cuda-12.4-x64.zip"),
                format!("{prefix}bin-win-cuda-12.4-x64.zip"),
            ],
            "vulkan" => vec![format!("{prefix}bin-win-vulkan-x64.zip")],
            "cpu" => vec![format!("{prefix}bin-win-cpu-x64.zip")],
            _ => vec![],
        }
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        match backend {
            "cuda" => vec![format!("{prefix}bin-ubuntu-cuda-12.4-x64.tar.gz")],
            "vulkan" => vec![format!("{prefix}bin-ubuntu-vulkan-x64.tar.gz")],
            "cpu" => vec![format!("{prefix}bin-ubuntu-x64.tar.gz")],
            _ => vec![],
        }
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        match backend {
            "cpu" => vec![format!("{prefix}bin-ubuntu-arm64.tar.gz")],
            _ => vec![],
        }
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        match backend {
            "metal" => vec![format!("{prefix}bin-macos-arm64.tar.gz")],
            _ => vec![],
        }
    } else {
        vec![]
    }
}

pub fn downloadable_bee_backends() -> Vec<String> {
    ["cuda", "vulkan", "metal", "cpu"]
        .iter()
        .filter(|backend| !bee_assets_for_backend(backend).is_empty())
        .map(|backend| backend.to_string())
        .collect()
}

pub fn bee_download_url(asset: &str) -> String {
    format!(
        "https://github.com/Anbeeld/beellama.cpp/releases/download/{BEELLAMA_RELEASE_TAG}/{asset}"
    )
}

pub fn bee_asset_sha256(asset: &str) -> Option<&'static str> {
    match asset {
        "beellama-v0.4.1-bin-macos-arm64.tar.gz" => {
            Some("2f33977bc987699a6c46bd08e870a689eaa57a620dfb537907d9c71102d066bb")
        }
        "beellama-v0.4.1-bin-ubuntu-arm64.tar.gz" => {
            Some("5bcc101afc4cd73cbe8832bb41bb47ebbae8a1eff52751929c1b0f8b73ef863a")
        }
        "beellama-v0.4.1-bin-ubuntu-cuda-12.4-x64.tar.gz" => {
            Some("acae04c79a9b2e48b8ea46ab0da369973b0481813003bc4623cd907d8b61bd8c")
        }
        "beellama-v0.4.1-bin-ubuntu-vulkan-x64.tar.gz" => {
            Some("2a33854daca03bcd0fd568789513fd430594861ce4139f44c9ccb5c9a09cbaac")
        }
        "beellama-v0.4.1-bin-ubuntu-x64.tar.gz" => {
            Some("19c67b1ac686af1e7e497f530f6b27aa144dd1ae20e3d6a9c4d5582b4d5be115")
        }
        "beellama-v0.4.1-bin-win-cpu-x64.zip" => {
            Some("d724f89533810686709528ab5eb7ecd9998c6aab529e68f6570f588e4ce0964b")
        }
        "beellama-v0.4.1-bin-win-cuda-12.4-x64.zip" => {
            Some("d1d6fb46f922ff1dd98aa7ba003ceb94b14aed7b6d30e9dbf6831c7ae4714949")
        }
        "beellama-v0.4.1-bin-win-vulkan-x64.zip" => {
            Some("c94479b0f2d675b1fd0370dba1b9e477bda41e592eca6cea8d213315078eea9c")
        }
        "beellama-v0.4.1-cudart-win-cuda-12.4-x64.zip" => {
            Some("8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6")
        }
        _ => None,
    }
}

pub fn download_url(asset: &str) -> String {
    format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{}/{}",
        LLAMA_RELEASE_TAG, asset
    )
}

/// Extract a downloaded archive using the platform's standard tool.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();

    let status = if name.ends_with(".zip") {
        if cfg!(target_os = "windows") {
            Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!(
                    "Expand-Archive -LiteralPath \"{}\" -DestinationPath \"{}\" -Force",
                    archive.display(),
                    dest.display()
                ))
                .status()
        } else {
            Command::new("unzip")
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(dest)
                .status()
        }
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()
    } else {
        bail!("unsupported archive format: {name}");
    };

    let status =
        status.with_context(|| format!("failed to run extractor for {}", archive.display()))?;
    if !status.success() {
        bail!("extraction failed for {}", archive.display());
    }
    Ok(())
}

/// Find the llama-server binary inside an extracted archive and copy its whole
/// directory into `backend_dir` (the .dll/.so siblings must come along).
pub fn install_backend_from_staging(staging: &Path, backend_dir: &Path) -> Result<()> {
    let exe_name = executable_name();
    let server = walkdir::WalkDir::new(staging)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_file() && e.file_name().to_string_lossy() == exe_name)
        .map(|e| e.path().to_path_buf())
        .ok_or_else(|| anyhow!("no {} found in the downloaded archive", exe_name))?;
    let src_dir = server
        .parent()
        .ok_or_else(|| anyhow!("invalid archive layout"))?;

    fs::create_dir_all(backend_dir)?;
    for entry in fs::read_dir(src_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let dest = backend_dir.join(entry.file_name());
            fs::copy(entry.path(), &dest)?;
            #[cfg(unix)]
            if entry.file_name().to_string_lossy() == exe_name {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&dest)?.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&dest, perms);
            }
        }
    }
    // Recreate the .so soname symlinks the file-only copy above dropped.
    ensure_sonames(backend_dir);
    Ok(())
}
