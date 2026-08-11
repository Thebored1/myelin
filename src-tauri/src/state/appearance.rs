use super::*;

const APPEARANCE_FILE_NAME: &str = "appearance.json";
const APPEARANCE_SCHEMA_VERSION: u32 = 1;
const BUILTIN_DARK: &str = "myelin-dark";
const BUILTIN_LIGHT: &str = "myelin-light";

fn appearance_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(APPEARANCE_FILE_NAME)
}

fn defaults() -> AppearanceSettings {
    AppearanceSettings {
        schema_version: APPEARANCE_SCHEMA_VERSION,
        active_theme_id: BUILTIN_DARK.to_string(),
        custom_themes: Vec::new(),
    }
}

fn load_appearance(app_data_dir: &Path) -> Result<AppearanceSettings> {
    let path = appearance_path(app_data_dir);
    if !path.exists() {
        return Ok(defaults());
    }
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read appearance settings {}", path.display()))?;
    let settings: AppearanceSettings = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse appearance settings {}", path.display()))?;
    validate_settings(&settings)?;
    Ok(settings)
}

fn validate_settings(settings: &AppearanceSettings) -> Result<()> {
    if settings.schema_version != APPEARANCE_SCHEMA_VERSION {
        return Err(anyhow!("unsupported appearance schema version {}", settings.schema_version));
    }
    let mut ids = std::collections::HashSet::new();
    for theme in &settings.custom_themes {
        validate_theme(theme)?;
        if !ids.insert(theme.id.clone()) {
            return Err(anyhow!("duplicate custom theme id {}", theme.id));
        }
    }
    if settings.custom_themes.iter().any(|theme| theme.id == settings.active_theme_id)
        || matches!(settings.active_theme_id.as_str(), BUILTIN_DARK | BUILTIN_LIGHT)
    {
        return Ok(());
    }
    Err(anyhow!("active theme {} does not exist", settings.active_theme_id))
}

fn validate_theme(theme: &ColorTheme) -> Result<()> {
    if theme.schema_version != APPEARANCE_SCHEMA_VERSION {
        return Err(anyhow!("unsupported theme schema version {}", theme.schema_version));
    }
    if theme.id.is_empty() || theme.id.len() > 80 || theme.id == BUILTIN_DARK || theme.id == BUILTIN_LIGHT {
        return Err(anyhow!("invalid or reserved custom theme id"));
    }
    if theme.name.trim().is_empty() || theme.name.chars().count() > 64 {
        return Err(anyhow!("theme name must contain between 1 and 64 characters"));
    }
    if !matches!(theme.mode.as_str(), "dark" | "light") {
        return Err(anyhow!("theme mode must be light or dark"));
    }
    if !matches!(theme.base_theme_id.as_str(), BUILTIN_DARK | BUILTIN_LIGHT) {
        return Err(anyhow!("theme base must be a built-in theme"));
    }

    let contract: serde_json::Value = serde_json::from_str(include_str!("../../../schemas/theme-token-contract.v1.json"))?;
    let definitions = contract
        .get("tokens")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("theme token contract is malformed"))?;
    for (id, value) in &theme.tokens {
        let definition = definitions.get(id).ok_or_else(|| anyhow!("unknown theme token {id}"))?;
        let raw = value.strip_prefix('#').ok_or_else(|| anyhow!("theme token {id} must be a hex color"))?;
        if !matches!(raw.len(), 6 | 8) || !raw.chars().all(|character| character.is_ascii_hexdigit()) {
            return Err(anyhow!("theme token {id} must be a hex color"));
        }
        if raw.len() == 8 && definition.get("alpha").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(anyhow!("theme token {id} does not allow transparency"));
        }
    }
    if let Some(palette) = &theme.palette {
        for (id, value) in palette {
            let raw = value.strip_prefix('#').ok_or_else(|| anyhow!("theme palette color {id} must be a hex color"))?;
            if !matches!(raw.len(), 6 | 8) || !raw.chars().all(|character| character.is_ascii_hexdigit()) {
                return Err(anyhow!("theme palette color {id} must be a hex color"));
            }
        }
    }
    Ok(())
}

fn save_appearance(app_data_dir: &Path, settings: &AppearanceSettings) -> Result<()> {
    validate_settings(settings)?;
    crate::persistence::atomic_write_json(&appearance_path(app_data_dir), settings)
}

fn publish_appearance(state: &AppState, settings: &AppearanceSettings) -> Result<()> {
    state.handle.emit("appearance://theme_changed", settings.clone())?;
    Ok(())
}

impl AppState {
    pub fn get_appearance_settings(&self) -> Result<AppearanceSettings> {
        let _guard = self.inner.persistence_lock.lock();
        load_appearance(&self.inner.app_data_dir)
    }

    pub fn save_color_theme(&self, theme: ColorTheme) -> Result<AppearanceSettings> {
        let _guard = self.inner.persistence_lock.lock();
        validate_theme(&theme)?;
        let mut settings = load_appearance(&self.inner.app_data_dir)?;
        settings.custom_themes.retain(|existing| existing.id != theme.id);
        settings.custom_themes.push(theme);
        save_appearance(&self.inner.app_data_dir, &settings)?;
        publish_appearance(self, &settings)?;
        Ok(settings)
    }

    pub fn delete_color_theme(&self, id: &str) -> Result<AppearanceSettings> {
        let _guard = self.inner.persistence_lock.lock();
        if matches!(id, BUILTIN_DARK | BUILTIN_LIGHT) {
            return Err(anyhow!("built-in themes cannot be deleted"));
        }
        let mut settings = load_appearance(&self.inner.app_data_dir)?;
        let original_len = settings.custom_themes.len();
        settings.custom_themes.retain(|theme| theme.id != id);
        if settings.custom_themes.len() == original_len {
            return Err(anyhow!("custom theme {id} was not found"));
        }
        if settings.active_theme_id == id {
            settings.active_theme_id = BUILTIN_DARK.to_string();
        }
        save_appearance(&self.inner.app_data_dir, &settings)?;
        publish_appearance(self, &settings)?;
        Ok(settings)
    }

    pub fn set_active_color_theme(&self, id: &str) -> Result<AppearanceSettings> {
        let _guard = self.inner.persistence_lock.lock();
        let mut settings = load_appearance(&self.inner.app_data_dir)?;
        if !matches!(id, BUILTIN_DARK | BUILTIN_LIGHT)
            && !settings.custom_themes.iter().any(|theme| theme.id == id)
        {
            return Err(anyhow!("theme {id} was not found"));
        }
        settings.active_theme_id = id.to_string();
        save_appearance(&self.inner.app_data_dir, &settings)?;
        publish_appearance(self, &settings)?;
        Ok(settings)
    }
}
