use serde::{Deserialize, Serialize};

/// Metadaten, die aus der Mod-JAR selbst gelesen wurden.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModMeta {
    /// "fabric", "quilt", "neoforge" oder "forge"
    pub loader: String,
    pub mod_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    /// Minecraft-Abhängigkeit laut Metadaten (z. B. "~1.21.1")
    pub minecraft: Option<String>,
}

/// Eine JAR im Quellordner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMod {
    pub file_name: String,
    pub path: String,
    pub size: u64,
    pub sha1: String,
    pub sha512: String,
    pub cf_fingerprint: u32,
    pub meta: Option<ModMeta>,
}

impl LocalMod {
    pub fn display_name(&self) -> String {
        self.meta
            .as_ref()
            .and_then(|m| m.name.clone().or_else(|| Some(m.mod_id.clone())))
            .unwrap_or_else(|| self.file_name.clone())
    }
}

/// Was der Nutzer als Ziel ausgewählt hat.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub game_version: String,
    pub loader: String,
    #[serde(default)]
    pub allow_beta: bool,
    #[serde(default)]
    pub allow_alpha: bool,
    #[serde(default = "default_true")]
    pub include_dependencies: bool,
    /// Bei Mods, die nicht per Hash gefunden wurden, über die Mod-ID suchen.
    #[serde(default = "default_true")]
    pub fallback_by_mod_id: bool,
}

fn default_true() -> bool {
    true
}

impl Target {
    /// Modrinth-Loader-Namen, die für das Ziel akzeptiert werden.
    pub fn modrinth_loaders(&self) -> Vec<String> {
        match self.loader.as_str() {
            "quilt" => vec!["quilt".into(), "fabric".into()],
            other => vec![other.to_string()],
        }
    }

    /// CurseForge-Loader-Bezeichnungen, wie sie in `gameVersions` auftauchen.
    pub fn curseforge_loaders(&self) -> Vec<String> {
        match self.loader.as_str() {
            "fabric" => vec!["Fabric".into()],
            "quilt" => vec!["Quilt".into(), "Fabric".into()],
            "neoforge" => vec!["NeoForge".into()],
            "forge" => vec!["Forge".into()],
            other => vec![other.to_string()],
        }
    }

    /// CurseForge `modLoaderType` für die Server-seitige Filterung (nur bei eindeutigem Loader).
    pub fn curseforge_loader_type(&self) -> Option<u32> {
        match self.loader.as_str() {
            "forge" => Some(1),
            "fabric" => Some(4),
            "neoforge" => Some(6),
            _ => None,
        }
    }

    pub fn channel_allowed(&self, channel: &str) -> bool {
        match channel {
            "release" => true,
            "beta" => self.allow_beta,
            "alpha" => self.allow_alpha,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Modrinth,
    Curseforge,
}

/// Eine konkrete Datei, die für das Ziel installiert werden kann.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub source: Source,
    pub project_id: String,
    pub project_name: String,
    pub project_url: String,
    pub icon_url: Option<String>,
    pub version_id: String,
    pub version_number: String,
    /// "release", "beta" oder "alpha"
    pub channel: String,
    pub file_name: String,
    /// `None`, wenn der Autor Downloads über Drittprogramme verbietet (CurseForge).
    pub download_url: Option<String>,
    pub sha1: Option<String>,
    pub size: u64,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub published: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Passende Version gefunden, kann heruntergeladen werden.
    Available,
    /// Die vorhandene Datei ist bereits die passende Version.
    UpToDate,
    /// Gefunden, aber Download nur über die Website (CurseForge-Sperre).
    ManualDownload,
    /// Projekt gefunden, aber keine Version für Ziel-Version/Loader.
    NotAvailable,
    /// Weder auf Modrinth noch auf CurseForge gefunden.
    NotFound,
    /// Fehler bei der Abfrage.
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownOn {
    pub source: Source,
    pub project_id: String,
    pub project_name: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MatchKind {
    /// Exakter Datei-Hash-Treffer – sicher.
    Hash,
    /// Treffer über Mod-ID/Slug – sollte geprüft werden.
    ModId,
    /// Als Abhängigkeit einer anderen Mod hinzugefügt.
    Dependency,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRow {
    pub key: String,
    pub name: String,
    pub local: Option<LocalMod>,
    pub required_by: Vec<String>,
    pub match_kind: MatchKind,
    pub status: Status,
    pub candidate: Option<Candidate>,
    pub known_on: Vec<KnownOn>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVersion {
    pub version: String,
    pub version_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallItem {
    pub key: String,
    pub file_name: String,
    pub url: String,
    pub sha1: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub key: String,
    pub file_name: String,
    /// "installed", "skipped", "failed", "cancelled"
    pub outcome: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: String,
    pub done: usize,
    pub total: usize,
    pub label: String,
}
