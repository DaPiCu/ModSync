pub mod curseforge;
pub mod http;
pub mod install;
pub mod modrinth;
pub mod models;
pub mod resolve;
pub mod scan;
pub mod settings;

use models::*;
use resolve::{CheckReport, Clients};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

#[derive(Default)]
struct AppState {
    cancel: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
}

/// Gibt `busy` beim Verlassen automatisch wieder frei.
struct BusyGuard(Arc<AtomicBool>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn acquire(state: &AppState) -> Result<BusyGuard, String> {
    if state.busy.swap(true, Ordering::SeqCst) {
        return Err("Es läuft bereits ein Vorgang.".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    Ok(BusyGuard(state.busy.clone()))
}

fn modrinth_base() -> String {
    std::env::var("MODSYNC_MODRINTH_BASE").unwrap_or_else(|_| modrinth::DEFAULT_BASE.into())
}

fn curseforge_base() -> String {
    std::env::var("MODSYNC_CURSEFORGE_BASE").unwrap_or_else(|_| curseforge::DEFAULT_BASE.into())
}

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("Einstellungsordner nicht gefunden: {e}"))
}

#[tauri::command]
fn load_settings(app: AppHandle) -> Result<settings::Settings, String> {
    Ok(settings::load(config_dir(&app)?))
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
    settings::save(config_dir(&app)?, &settings)
}

#[tauri::command]
async fn scan_mods(dir: String) -> Result<Vec<LocalMod>, String> {
    tokio::task::spawn_blocking(move || scan::scan_folder(&PathBuf::from(dir)))
        .await
        .map_err(|e| format!("Scan fehlgeschlagen: {e}"))?
}

#[tauri::command]
async fn game_versions() -> Result<Vec<GameVersion>, String> {
    modrinth::Modrinth::new(modrinth_base()).game_versions().await
}

#[tauri::command]
async fn check_mods(
    app: AppHandle,
    state: State<'_, AppState>,
    mods: Vec<LocalMod>,
    target: Target,
) -> Result<CheckReport, String> {
    let _guard = acquire(&state)?;
    let clients = Clients {
        modrinth: modrinth::Modrinth::new(modrinth_base()),
        curseforge: settings::get_cf_key()
            .map(|k| curseforge::CurseForge::new(curseforge_base(), k)),
    };
    let cancel = state.cancel.clone();
    resolve::check(
        mods,
        &target,
        &clients,
        |p| {
            let _ = app.emit("progress", p);
        },
        &cancel,
    )
    .await
}

#[tauri::command]
async fn install_mods(
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<InstallItem>,
    target_dir: String,
) -> Result<Vec<InstallResult>, String> {
    if target_dir.trim().is_empty() {
        return Err("Bitte einen Zielordner auswählen.".into());
    }
    let _guard = acquire(&state)?;
    let cancel = state.cancel.clone();
    install::install_all(
        items,
        &PathBuf::from(target_dir),
        |p| {
            let _ = app.emit("progress", p);
        },
        &cancel,
    )
    .await
}

#[tauri::command]
fn cancel_operation(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn cf_key_status() -> bool {
    settings::get_cf_key().is_some()
}

#[tauri::command]
fn set_cf_key(key: Option<String>) -> Result<(), String> {
    settings::set_cf_key(key)
}

/// Testet den übergebenen Key oder – wenn leer – den gespeicherten.
#[tauri::command]
async fn test_cf_key(key: Option<String>) -> Result<(), String> {
    let key = key
        .filter(|k| !k.trim().is_empty())
        .or_else(settings::get_cf_key)
        .ok_or("Kein API-Key angegeben.")?;
    curseforge::CurseForge::new(curseforge_base(), key.trim())
        .test_key()
        .await
}

#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("Nur Web-Adressen können geöffnet werden.".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Link kann nicht geöffnet werden: {e}"))
}

#[tauri::command]
fn open_folder(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("Ordner nicht gefunden: {path}"));
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("Ordner kann nicht geöffnet werden: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            scan_mods,
            game_versions,
            check_mods,
            install_mods,
            cancel_operation,
            cf_key_status,
            set_cf_key,
            test_cf_key,
            open_url,
            open_folder
        ])
        .run(tauri::generate_context!())
        .expect("ModSync konnte nicht gestartet werden");
}
