use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::Manager;

/// Impostazioni dell'app stessa, separate da `rclone.conf` (i remote) e dai
/// file per-funzionalità (`mounts.toml`, `jobs.toml`, ...): oggi il solo
/// campo è "avvia ridotta a icona", ma un file dedicato evita di dover
/// scegliere in futuro a quale altro file agganciare una nuova preferenza
/// globale.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// Se `true`, la finestra principale resta nascosta in tray all'avvio
    /// invece di comparire subito — l'avvio "all'accensione del computer"
    /// vero e proprio è gestito da `tauri-plugin-autostart`, che non ha
    /// bisogno di persistenza propria (lo stato è già quello del sistema
    /// operativo, interrogabile con `isEnabled()`).
    #[serde(default)]
    pub start_minimized: bool,
}

fn settings_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.toml")
}

pub(crate) fn load_from_dir(config_dir: &Path) -> AppSettings {
    let path = settings_file_path(config_dir);
    let Ok(content) = std::fs::read_to_string(&path) else { return AppSettings::default() };
    toml::from_str(&content).unwrap_or_default()
}

fn save_to_dir(config_dir: &Path, settings: &AppSettings) -> Result<(), String> {
    std::fs::create_dir_all(config_dir).map_err(|e| format!("impossibile creare '{}': {e}", config_dir.display()))?;
    let content = toml::to_string_pretty(settings).map_err(|e| format!("impossibile serializzare le impostazioni: {e}"))?;
    crate::fs_atomic::write_atomically(&settings_file_path(config_dir), &content)
}

#[tauri::command]
pub fn get_app_settings(app: tauri::AppHandle) -> Result<AppSettings, String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(load_from_dir(&config_dir))
}

#[tauri::command]
pub fn set_start_minimized(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let mut settings = load_from_dir(&config_dir);
    settings.start_minimized = enabled;
    save_to_dir(&config_dir, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rcd::tests::TempDir;

    #[test]
    fn load_from_dir_returns_defaults_when_no_file_exists_yet() {
        let dir = TempDir::new("app-settings-missing");
        let settings = load_from_dir(&dir.path);
        assert!(!settings.start_minimized);
    }

    #[test]
    fn set_start_minimized_persists_across_reloads() {
        let dir = TempDir::new("app-settings-persist");
        std::fs::create_dir_all(&dir.path).unwrap();
        save_to_dir(&dir.path, &AppSettings { start_minimized: true }).unwrap();

        let settings = load_from_dir(&dir.path);
        assert!(settings.start_minimized);
    }

    #[test]
    fn set_start_minimized_can_be_turned_back_off() {
        let dir = TempDir::new("app-settings-toggle-off");
        std::fs::create_dir_all(&dir.path).unwrap();
        save_to_dir(&dir.path, &AppSettings { start_minimized: true }).unwrap();
        save_to_dir(&dir.path, &AppSettings { start_minimized: false }).unwrap();

        let settings = load_from_dir(&dir.path);
        assert!(!settings.start_minimized);
    }
}
