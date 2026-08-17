//! Optional, failure-tolerant Tesseract OCR for low-text PDF pages.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrSettings {
    pub auto_low_text_pages: bool,
    pub executable_path: Option<String>,
    pub language: String,
}
impl Default for OcrSettings {
    fn default() -> Self {
        Self {
            auto_low_text_pages: true,
            executable_path: None,
            language: "eng".into(),
        }
    }
}
impl OcrSettings {
    pub fn is_default(&self) -> bool {
        self.auto_low_text_pages && self.executable_path.is_none() && self.language == "eng"
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.language.trim().is_empty()
            || self.language.len() > 32
            || !self
                .language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+')
        {
            return Err(
                "OCR language must be a short Tesseract language code (for example, eng).".into(),
            );
        }
        if let Some(path) = &self.executable_path {
            if path.trim().is_empty() {
                return Err("OCR executable path cannot be empty when provided.".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrStatus {
    pub available: bool,
    pub executable_path: Option<String>,
    pub configured_language: String,
    pub auto_low_text_pages: bool,
    pub language_available: bool,
    pub version: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPageResult {
    pub page_number: u32,
    pub text: String,
    pub confidence: Option<f32>,
    pub word_count: usize,
    pub mode: String,
}

pub fn status(settings: &OcrSettings) -> OcrStatus {
    OcrStatus {
        available: false,
        executable_path: settings.executable_path.clone(),
        configured_language: settings.language.clone(),
        auto_low_text_pages: settings.auto_low_text_pages,
        language_available: false,
        version: None,
        warning: Some("Bundled OCR is not enabled yet; PDF extraction uses native text.".into()),
    }
}

pub fn recognize_png(
    png: &[u8],
    page_number: u32,
    settings: &OcrSettings,
) -> Result<OcrPageResult, String> {
    let _ = (png, page_number, settings);
    Err("Bundled OCR is not enabled yet; native PDF extraction is active.".into())
    /*
    let state = status(settings);
    if !state.available { return Err(state.warning.unwrap_or_else(|| "Tesseract is unavailable".into())); }
    if !state.language_available { return Err(state.warning.unwrap_or_else(|| "configured OCR language is unavailable".into())); }
    let dir = tempfile::Builder::new().prefix("myelin-ocr-").tempdir().map_err(|e| e.to_string())?;
    let image = dir.path().join("page.png");
    std::fs::File::create(&image).and_then(|mut file| file.write_all(png)).map_err(|e| e.to_string())?;
    let output = Command::new(executable(settings)).arg(&image).arg("stdout").arg("-l").arg(&settings.language).arg("tsv").output().map_err(|e| format!("failed to start Tesseract: {e}"))?;
    if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_string()); }
    let mut lines = std::collections::BTreeMap::<(String, String, String), Vec<String>>::new();
    let mut confidence = Vec::new();
    for row in String::from_utf8_lossy(&output.stdout).lines().skip(1) {
        let fields: Vec<_> = row.split('\t').collect();
        if fields.len() < 12 { continue; }
        let text = fields[11].trim(); if text.is_empty() { continue; }
        if let Ok(value) = fields[10].parse::<f32>() { if value >= 0.0 && value.is_finite() { confidence.push(value); } }
        lines.entry((fields[2].into(), fields[3].into(), fields[4].into())).or_default().push(text.into());
    }
    let text = lines.into_values().map(|words| words.join(" ")).collect::<Vec<_>>().join("\n");
    Ok(OcrPageResult { page_number, word_count: text.split_whitespace().count(), confidence: (!confidence.is_empty()).then(|| confidence.iter().sum::<f32>() / confidence.len() as f32), text, mode: "ocr".into() }) */
}
