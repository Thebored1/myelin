use super::process::has_fatal_startup_error;
use super::{
    bee_asset_sha256, bee_assets_for_backend, cache_reuse_tokens, fit_ngl, has_tensor_override,
    normalize_engine, resolve_embedding_executable,
};

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

#[test]
fn bee_is_opt_in_and_every_platform_asset_has_a_pinned_digest() {
    assert_eq!(normalize_engine(None), "llama_cpp");
    assert_eq!(normalize_engine(Some("beellama")), "beellama");
    for backend in ["cuda", "vulkan", "metal", "cpu"] {
        for asset in bee_assets_for_backend(backend) {
            assert!(
                bee_asset_sha256(&asset).is_some(),
                "missing Bee checksum for {asset}"
            );
        }
    }
}

#[test]
fn picks_discrete_gpu_on_hybrid_keeps_igpu_alone() {
    use super::{discrete_device_id, integrated_device_id, DeviceInfo};
    let dev = |id: &str, name: &str| DeviceInfo {
        id: id.into(),
        name: name.into(),
        backend: "vulkan".into(),
    };
    // Hybrid laptop: prefer the discrete NVIDIA, identify the Intel iGPU.
    let hybrid = vec![
        dev("Vulkan0", "Intel(R) UHD Graphics"),
        dev("Vulkan1", "NVIDIA GeForce RTX 2050"),
    ];
    assert_eq!(discrete_device_id(&hybrid).as_deref(), Some("Vulkan1"));
    assert_eq!(integrated_device_id(&hybrid).as_deref(), Some("Vulkan0"));
    // Single iGPU (the Linux box): no discrete choice — use the only device.
    let single = vec![dev("Vulkan0", "AMD Radeon Graphics (RADV RENOIR)")];
    assert_eq!(discrete_device_id(&single), None);
    assert_eq!(integrated_device_id(&single).as_deref(), Some("Vulkan0"));
}

#[test]
fn integrated_budget_reserves_kv_then_caps_at_gtt() {
    use super::integrated_weight_budget;
    let g = GIB;
    // 16 GiB RAM, 2 GiB KV, 1.5 GiB reserve, 7 GiB GTT → RAM-KV-reserve=12.5,
    // capped at GTT → 7 GiB.
    assert_eq!(
        integrated_weight_budget(16 * g, 2 * g, 7 * g, 3 * g / 2),
        7 * g
    );
    // Low RAM dominates: 6 GiB RAM, 3 GiB KV, 1.5 reserve → 1.5 GiB (< 7 GTT).
    assert_eq!(
        integrated_weight_budget(6 * g, 3 * g, 7 * g, 3 * g / 2),
        3 * g / 2
    );
    // Over-committed RAM → 0 (fall to CPU), never underflows.
    assert_eq!(integrated_weight_budget(2 * g, 3 * g, 7 * g, 3 * g / 2), 0);
}

#[test]
fn fit_ngl_full_when_it_fits() {
    // 4 GiB free, 856 MiB model, 40 layers → all fit.
    assert_eq!(fit_ngl(4 * GIB, 856 * MIB, 40), 40);
}

#[test]
fn fit_ngl_partial_split() {
    // 1 GiB free, 2 GiB model, 40 layers → a partial GPU/CPU split.
    let ngl = fit_ngl(GIB, 2 * GIB, 40);
    assert!(ngl > 0 && ngl < 40, "expected partial split, got {ngl}");
}

#[test]
fn fit_ngl_zero_when_nothing_fits() {
    assert_eq!(fit_ngl(64 * MIB, 4 * GIB, 40), 0);
}

#[test]
fn fit_ngl_unknown_geometry_requests_all() {
    assert_eq!(fit_ngl(GIB, 0, 40), 999);
    assert_eq!(fit_ngl(GIB, 1024, 0), 999);
}

#[test]
fn detects_user_tensor_override_spellings() {
    assert!(has_tensor_override(&[
        "--override-tensor".into(),
        ".*attn.*=GPU".into()
    ]));
    assert!(has_tensor_override(&["-ot=.*ffn.*=CPU".into()]));
    assert!(has_tensor_override(&[
        "--override-tensor=.*ffn.*=CPU".into()
    ]));
    assert!(!has_tensor_override(&["--threads".into(), "4".into()]));
}

#[test]
fn bee_uses_shorter_cache_reuse_chunks() {
    assert_eq!(cache_reuse_tokens("beellama"), "64");
    assert_eq!(cache_reuse_tokens("llama_cpp"), "256");
}

#[test]
fn detects_fatal_startup_errors_but_not_progress_lines() {
    assert!(has_fatal_startup_error(&[
        "error: unknown argument --bad".into()
    ]));
    assert!(has_fatal_startup_error(&[
        "CUDA error: out of memory".into()
    ]));
    assert!(!has_fatal_startup_error(&[
        "llama_model_loader: loading tensors".into(),
        "warning: using CPU fallback".into(),
    ]));
}

#[test]
fn embedding_runtime_uses_stock_cpu_and_ignores_bee() {
    let dir = tempfile::tempdir().unwrap();
    let stock = dir
        .path()
        .join("bin")
        .join("cpu")
        .join(super::executable_name());
    let bee = dir
        .path()
        .join("bin")
        .join("bee")
        .join("cpu")
        .join(super::executable_name());
    std::fs::create_dir_all(stock.parent().unwrap()).unwrap();
    std::fs::create_dir_all(bee.parent().unwrap()).unwrap();
    std::fs::write(&stock, b"stock").unwrap();
    std::fs::write(&bee, b"bee").unwrap();

    assert_eq!(resolve_embedding_executable(dir.path()).unwrap(), stock);
}

#[test]
fn embedding_runtime_reports_missing_stock_cpu_binary() {
    let dir = tempfile::tempdir().unwrap();
    let error = resolve_embedding_executable(dir.path()).unwrap_err();
    assert!(error.to_string().contains("stock CPU embedding server"));
}
