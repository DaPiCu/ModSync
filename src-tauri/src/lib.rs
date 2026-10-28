pub mod builder;
pub mod curseforge;
pub mod gradle;
pub mod http;
pub mod install;
pub mod jdk;
pub mod modrinth;
pub mod models;
pub mod overrides;
pub mod patcher;
pub mod resolve;
pub mod scan;
pub mod settings;
pub mod sources;
pub mod template;

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
    child: gradle::ChildSlot,
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
    if let Some(pid) = *state.child.lock().unwrap() {
        gradle::kill_tree(pid);
    }
}

// ---- Selbst bauen ------------------------------------------------------------------------

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|e| format!("Datenordner nicht gefunden: {e}"))
}

fn builds_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("builds"))
}

fn jdks_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("jdks"))
}

#[tauri::command]
async fn build_branches(url: String, target: String) -> Result<sources::BranchInfo, String> {
    let repo = sources::parse_github(&url)
        .ok_or("Nur GitHub-Links werden unterstützt (https://github.com/besitzer/repo).")?;
    sources::branches(&builder::Endpoints::default().github_api, &repo, &target).await
}

#[tauri::command]
async fn build_prepare(
    app: AppHandle,
    state: State<'_, AppState>,
    request: builder::PrepareRequest,
) -> Result<builder::Prepared, String> {
    let _guard = acquire(&state)?;
    let cancel = state.cancel.clone();
    builder::prepare(&request, &builds_dir(&app)?, &builder::Endpoints::default(), &cancel).await
}

#[tauri::command]
fn build_find_jdk(app: AppHandle, major: u32) -> Result<Option<jdk::Jdk>, String> {
    Ok(jdk::pick(&jdk::find_all(&jdks_dir(&app)?), major))
}

#[tauri::command]
async fn build_download_jdk(
    app: AppHandle,
    state: State<'_, AppState>,
    major: u32,
) -> Result<jdk::Jdk, String> {
    let _guard = acquire(&state)?;
    let cancel = state.cancel.clone();
    let dir = jdks_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let base = std::env::var("MODSYNC_ADOPTIUM_BASE").unwrap_or_else(|_| jdk::DEFAULT_ADOPTIUM.into());
    let app2 = app.clone();
    jdk::download(
        &base,
        major,
        &dir,
        move |done, total| {
            let _ = app2.emit("jdk-progress", (done, total));
        },
        &cancel,
    )
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildRunResult {
    #[serde(flatten)]
    outcome: gradle::BuildOutcome,
    output_jar: Option<String>,
    hint: Option<String>,
}

#[tauri::command]
async fn build_run(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    java_home: String,
    target: String,
) -> Result<BuildRunResult, String> {
    if id.contains(['/', '\\']) || id.contains("..") {
        return Err("Ungültige Build-ID.".into());
    }
    let work = builds_dir(&app)?.join(&id);
    let project = work.join("src");
    if !project.is_dir() {
        return Err("Arbeitskopie nicht gefunden – bitte neu vorbereiten.".into());
    }
    let java = PathBuf::from(&java_home);
    if jdk::inspect(&java).is_none() {
        return Err("Das gewählte JDK ist ungültig.".into());
    }
    let _guard = acquire(&state)?;
    let cancel = state.cancel.clone();
    let app2 = app.clone();
    let outcome = gradle::run(
        &project,
        &java,
        &work.join("build.log"),
        move |line| {
            let _ = app2.emit("build-log", line);
        },
        &cancel,
        state.child.clone(),
    )
    .await?;
    let output_jar = match &outcome.jar {
        Some(j) if outcome.success => Some(
            builder::collect_output(&PathBuf::from(j), &work, &target)?
                .to_string_lossy()
                .to_string(),
        ),
        _ => None,
    };
    let hint = if outcome.success { None } else { gradle::hint(&outcome) };
    Ok(BuildRunResult { outcome, output_jar, hint })
}

/// Kopiert eine gebaute JAR in den Zielordner (ohne zu überschreiben).
#[tauri::command]
fn build_install(app: AppHandle, jar: String, target_dir: String) -> Result<String, String> {
    let jar = PathBuf::from(jar);
    let builds = builds_dir(&app)?;
    let canon = jar.canonicalize().map_err(|_| "Gebaute Datei nicht gefunden.".to_string())?;
    let canon_builds = builds.canonicalize().map_err(|e| e.to_string())?;
    if !canon.starts_with(&canon_builds) {
        return Err("Nur von ModSync gebaute Dateien können installiert werden.".into());
    }
    if target_dir.trim().is_empty() {
        return Err("Bitte einen Zielordner auswählen.".into());
    }
    let dir = PathBuf::from(target_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Zielordner: {e}"))?;
    let dest = dir.join(canon.file_name().unwrap());
    if dest.exists() {
        return Err("Im Zielordner liegt schon eine Datei mit diesem Namen – nicht überschrieben.".into());
    }
    std::fs::copy(&canon, &dest).map_err(|e| format!("Kopieren fehlgeschlagen: {e}"))?;
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
fn override_check(mod_minecraft: Option<String>, target: String) -> Result<(), String> {
    overrides::feasibility(mod_minecraft.as_deref(), &target)
}

#[tauri::command]
fn override_install(
    jar: String,
    mod_id: String,
    mod_minecraft: Option<String>,
    target: String,
    target_dir: String,
) -> Result<overrides::OverrideResult, String> {
    overrides::feasibility(mod_minecraft.as_deref(), &target)?;
    if target_dir.trim().is_empty() {
        return Err("Bitte einen Zielordner auswählen.".into());
    }
    let jar = PathBuf::from(jar);
    if !jar.is_file() || jar.extension().map(|e| e != "jar").unwrap_or(true) {
        return Err("Original-JAR nicht gefunden.".into());
    }
    overrides::install(&jar, &mod_id, &target, &PathBuf::from(target_dir))
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
            open_folder,
            build_branches,
            build_prepare,
            build_find_jdk,
            build_download_jdk,
            build_run,
            build_install,
            override_check,
            override_install
        ])
        .run(tauri::generate_context!())
        .expect("ModSync konnte nicht gestartet werden");
}
