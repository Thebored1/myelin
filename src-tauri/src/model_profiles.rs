//! Data-driven model handling: per-model knowledge (chat-template fixes,
//! tool-calling support, role, sampling) lives in a registry, not in code, and
//! resolves in three tiers — GGUF metadata (auto) ← bundled profiles (curated,
//! the "verified" list) ← user profiles (advanced overrides). Adding a model
//! becomes editing JSON, not Rust.

use crate::gguf::GgufInfo;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One registry entry. Matched against a model by GGUF `architecture` (exact,
/// case-insensitive) or a filename substring (`namePattern`). Every override
/// field is optional so a profile only states what it changes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    #[serde(default)]
    pub name: String,
    /// Match by GGUF general.architecture (e.g. "granitehybrid"), case-insensitive.
    #[serde(default)]
    pub architecture: Option<String>,
    /// ...or by a case-insensitive substring of the model file name.
    #[serde(default)]
    pub name_pattern: Option<String>,
    /// "chat" (default) or "embed".
    #[serde(default)]
    pub role: Option<String>,
    /// Chat-template override: the builtin id "lfm2", or an absolute file path.
    #[serde(default)]
    pub chat_template: Option<String>,
    /// Whether the model reliably does tool-calling. None → derive/probe.
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Curated + tested by us (UI "verified" badge) vs auto/experimental.
    #[serde(default)]
    pub verified: bool,
    /// One-line note for the compatibility list.
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    Chat,
    Embed,
}

/// The merged result the launcher uses: GGUF-derived defaults overlaid by the
/// best-matching bundled profile, then the best-matching user profile.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub profile_name: Option<String>,
    pub role: ModelRole,
    pub chat_template: Option<String>,
    pub supports_tools: Option<bool>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub is_recurrent_or_hybrid: bool,
    pub verified: bool,
}

impl ResolvedProfile {
    fn from_gguf(gguf: Option<&GgufInfo>) -> Self {
        ResolvedProfile {
            profile_name: None,
            role: ModelRole::Chat,
            chat_template: None,
            supports_tools: None,
            temperature: None,
            top_p: None,
            is_recurrent_or_hybrid: gguf.map(|g| g.is_recurrent_or_hybrid()).unwrap_or(false),
            verified: false,
        }
    }

    /// Overlay the Some() fields of a profile (later tiers win).
    fn apply(&mut self, p: &ModelProfile) {
        if !p.name.is_empty() {
            self.profile_name = Some(p.name.clone());
        }
        if let Some(role) = p.role.as_deref() {
            self.role = if role.eq_ignore_ascii_case("embed") {
                ModelRole::Embed
            } else {
                ModelRole::Chat
            };
        }
        if p.chat_template.is_some() {
            self.chat_template = p.chat_template.clone();
        }
        if p.supports_tools.is_some() {
            self.supports_tools = p.supports_tools;
        }
        if p.temperature.is_some() {
            self.temperature = p.temperature;
        }
        if p.top_p.is_some() {
            self.top_p = p.top_p;
        }
        self.verified = p.verified;
    }
}

const BUNDLED_JSON: &str = include_str!("../model-profiles.json");

/// Profiles shipped with the app (the curated, "verified" list).
pub fn bundled_profiles() -> Vec<ModelProfile> {
    serde_json::from_str(BUNDLED_JSON).unwrap_or_default()
}

/// User-added profiles from `<app_data>/model-profiles.json` (optional).
pub fn user_profiles(app_data_dir: &Path) -> Vec<ModelProfile> {
    std::fs::read_to_string(app_data_dir.join("model-profiles.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Every known profile (bundled then user), for the compatibility-list UI.
pub fn all_profiles(app_data_dir: &Path) -> Vec<ModelProfile> {
    let mut all = bundled_profiles();
    all.extend(user_profiles(app_data_dir));
    all
}

/// Match strength: an architecture match (exact, case-insensitive) is stronger
/// than a filename-substring match; 0 = no match.
fn match_strength(p: &ModelProfile, arch: Option<&str>, filename: &str) -> u8 {
    if let (Some(pa), Some(a)) = (p.architecture.as_deref(), arch) {
        if a.eq_ignore_ascii_case(pa) {
            return 2;
        }
    }
    if let Some(pat) = p.name_pattern.as_deref() {
        if !pat.is_empty() && filename.to_lowercase().contains(&pat.to_lowercase()) {
            return 1;
        }
    }
    0
}

fn best_match<'a>(
    profiles: &'a [ModelProfile],
    arch: Option<&str>,
    filename: &str,
) -> Option<&'a ModelProfile> {
    profiles
        .iter()
        .map(|p| (match_strength(p, arch, filename), p))
        .filter(|(s, _)| *s > 0)
        .max_by_key(|(s, _)| *s)
        .map(|(_, p)| p)
}

/// Resolve the effective profile for a model: GGUF defaults ← bundled ← user.
pub fn resolve(
    app_data_dir: &Path,
    gguf: Option<&GgufInfo>,
    model_filename: &str,
) -> ResolvedProfile {
    let arch = gguf.and_then(|g| g.architecture.as_deref());
    let mut resolved = ResolvedProfile::from_gguf(gguf);
    if let Some(p) = best_match(&bundled_profiles(), arch, model_filename) {
        resolved.apply(p);
    }
    if let Some(p) = best_match(&user_profiles(app_data_dir), arch, model_filename) {
        resolved.apply(p);
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gguf(arch: &str) -> GgufInfo {
        GgufInfo {
            architecture: Some(arch.into()),
            ..Default::default()
        }
    }
    fn nowhere() -> &'static Path {
        Path::new("/nonexistent-myelin-test-dir")
    }

    #[test]
    fn bundled_parses_nonempty() {
        assert!(!bundled_profiles().is_empty());
    }

    #[test]
    fn granite_is_verified_chat_with_tools() {
        let r = resolve(nowhere(), Some(&gguf("granitehybrid")), "granite-4.0-h-1b-Q4_K_M.gguf");
        assert_eq!(r.role, ModelRole::Chat);
        assert_eq!(r.supports_tools, Some(true));
        assert!(r.verified);
        assert!(r.is_recurrent_or_hybrid);
        assert!(r.chat_template.is_none());
    }

    #[test]
    fn lfm2_gets_template_override() {
        let r = resolve(nowhere(), Some(&gguf("lfm2")), "LFM2.5-1.2B-Instruct-Q4_K_M.gguf");
        assert_eq!(r.chat_template.as_deref(), Some("lfm2"));
        assert!(r.is_recurrent_or_hybrid);
    }

    #[test]
    fn nomic_matches_by_filename_as_embed() {
        let r = resolve(nowhere(), Some(&gguf("nomic-bert")), "nomic-embed-text-v1.5.Q4_K_M.gguf");
        assert_eq!(r.role, ModelRole::Embed);
    }

    #[test]
    fn unknown_model_defaults_to_chat() {
        let r = resolve(nowhere(), Some(&gguf("qwen2")), "some-random-model.gguf");
        assert_eq!(r.role, ModelRole::Chat);
        assert_eq!(r.supports_tools, None);
        assert!(!r.verified);
        assert!(r.chat_template.is_none());
    }
}
