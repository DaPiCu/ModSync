use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "net.dapicu.modsync";
const KEYRING_USER: &str = "curseforge-api-key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub source_dir: String,
    pub target_dir: String,
    pub game_version: String,
    pub loader: String,
    pub allow_beta: bool,
    pub allow_alpha: bool,
    pub include_dependencies: bool,
    pub fallback_by_mod_id: bool,
    pub show_snapshots: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_dir: String::new(),
            target_dir: String::new(),
            game_version: String::new(),
            loader: "fabric".into(),
            allow_beta: false,
            allow_alpha: false,
            include_dependencies: true,
            fallback_by_mod_id: true,
            show_snapshots: false,
        }
    }
}

pub fn path(config_dir: PathBuf) -> PathBuf {
    config_dir.join("settings.json")
}

pub fn load(config_dir: PathBuf) -> Settings {
    std::fs::read_to_string(path(config_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(config_dir: PathBuf, s: &Settings) -> Result<(), String> {
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("Einstellungsordner kann nicht angelegt werden: {e}"))?;
    let json = serde_json::to_string_pretty(s).unwrap();
    std::fs::write(path(config_dir), json)
        .map_err(|e| format!("Einstellungen können nicht gespeichert werden: {e}"))
}

// ---- CurseForge-API-Key im Windows-Anmeldeinformationsspeicher -----------------------------

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("Sicherer Speicher nicht verfügbar: {e}"))
}

pub fn get_cf_key() -> Option<String> {
    if let Ok(k) = std::env::var("MODSYNC_CURSEFORGE_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    entry()
        .ok()?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

pub fn set_cf_key(key: Option<String>) -> Result<(), String> {
    let e = entry()?;
    match key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        Some(k) => e
            .set_password(&k)
            .map_err(|err| format!("API-Key konnte nicht gespeichert werden: {err}")),
        None => match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(format!("API-Key konnte nicht gelöscht werden: {err}")),
        },
    }
}
