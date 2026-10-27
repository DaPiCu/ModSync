use crate::http;
use crate::models::{Candidate, GameVersion, Source, Target};
use serde::Deserialize;
use std::collections::HashMap;

pub const DEFAULT_BASE: &str = "https://api.modrinth.com";
const SERVICE: &str = "Modrinth";

#[derive(Debug, Clone, Deserialize)]
pub struct MrFile {
    pub hashes: HashMap<String, String>,
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MrDependency {
    pub version_id: Option<String>,
    pub project_id: Option<String>,
    pub dependency_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MrVersion {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub files: Vec<MrFile>,
    #[serde(default)]
    pub dependencies: Vec<MrDependency>,
}

impl MrVersion {
    pub fn primary_file(&self) -> Option<&MrFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.iter().find(|f| f.filename.ends_with(".jar")))
            .or_else(|| self.files.first())
    }

    pub fn required_dependency_refs(&self) -> Vec<(Option<String>, Option<String>)> {
        self.dependencies
            .iter()
            .filter(|d| d.dependency_type == "required")
            .map(|d| (d.project_id.clone(), d.version_id.clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MrProject {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub icon_url: Option<String>,
    #[serde(default)]
    pub project_type: String,
}

impl MrProject {
    pub fn url(&self) -> String {
        format!("https://modrinth.com/mod/{}", self.slug)
    }
}

#[derive(Debug, Deserialize)]
struct MrGameVersionTag {
    version: String,
    version_type: String,
}

#[derive(Clone)]
pub struct Modrinth {
    http: reqwest::Client,
    base: String,
}

impl Modrinth {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            http: http::client(),
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Option<T>, String> {
        let resp = req.send().await.map_err(|e| http::describe(SERVICE, &e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(http::describe_status(SERVICE, status));
        }
        resp.json::<T>()
            .await
            .map(Some)
            .map_err(|e| http::describe(SERVICE, &e))
    }

    /// Identifiziert Dateien per SHA-512. Ergebnis: Hash → Version.
    pub async fn versions_by_hashes(
        &self,
        sha512: &[String],
    ) -> Result<HashMap<String, MrVersion>, String> {
        let mut out = HashMap::new();
        for chunk in sha512.chunks(200) {
            let body = serde_json::json!({ "hashes": chunk, "algorithm": "sha512" });
            let req = self
                .http
                .post(format!("{}/v2/version_files", self.base))
                .json(&body);
            if let Some(map) = self.send::<HashMap<String, MrVersion>>(req).await? {
                out.extend(map);
            }
        }
        Ok(out)
    }

    pub async fn projects(&self, ids: &[String]) -> Result<HashMap<String, MrProject>, String> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(100) {
            let ids_json = serde_json::to_string(chunk).unwrap();
            let req = self
                .http
                .get(format!("{}/v2/projects", self.base))
                .query(&[("ids", ids_json)]);
            if let Some(list) = self.send::<Vec<MrProject>>(req).await? {
                for p in list {
                    out.insert(p.id.clone(), p);
                }
            }
        }
        Ok(out)
    }

    /// Projekt per ID oder Slug (für den Fallback über die Mod-ID).
    pub async fn project(&self, id_or_slug: &str) -> Result<Option<MrProject>, String> {
        let req = self.http.get(format!(
            "{}/v2/project/{}",
            self.base,
            urlencode(id_or_slug)
        ));
        self.send::<MrProject>(req).await
    }

    pub async fn version(&self, id: &str) -> Result<Option<MrVersion>, String> {
        let req = self
            .http
            .get(format!("{}/v2/version/{}", self.base, urlencode(id)));
        self.send::<MrVersion>(req).await
    }

    /// Alle Versionen eines Projekts, serverseitig nach Loader und Spielversion gefiltert.
    pub async fn project_versions(
        &self,
        project_id: &str,
        target: &Target,
    ) -> Result<Vec<MrVersion>, String> {
        let loaders = serde_json::to_string(&target.modrinth_loaders()).unwrap();
        let versions = serde_json::to_string(&[&target.game_version]).unwrap();
        let req = self
            .http
            .get(format!(
                "{}/v2/project/{}/version",
                self.base,
                urlencode(project_id)
            ))
            .query(&[("loaders", loaders), ("game_versions", versions)]);
        Ok(self.send::<Vec<MrVersion>>(req).await?.unwrap_or_default())
    }

    pub async fn game_versions(&self) -> Result<Vec<GameVersion>, String> {
        let req = self.http.get(format!("{}/v2/tag/game_version", self.base));
        let tags = self
            .send::<Vec<MrGameVersionTag>>(req)
            .await?
            .unwrap_or_default();
        Ok(tags
            .into_iter()
            .map(|t| GameVersion {
                version: t.version,
                version_type: t.version_type,
            })
            .collect())
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Ergebnis der Versionsauswahl.
pub enum Pick {
    Found(MrVersion),
    /// Es gibt passende Versionen, aber nur in nicht erlaubten Kanälen (Beta/Alpha).
    OnlyChannels(Vec<String>),
    None,
}

/// Wählt die neueste passende Version. Prüft clientseitig erneut Spielversion, Loader und Kanal.
pub fn pick_version(mut versions: Vec<MrVersion>, target: &Target) -> Pick {
    let loaders = target.modrinth_loaders();
    versions.retain(|v| {
        v.game_versions.iter().any(|g| g == &target.game_version)
            && v.loaders.iter().any(|l| loaders.contains(l))
            && v.primary_file().is_some()
    });
    if versions.is_empty() {
        return Pick::None;
    }
    // Bevorzugter Loader zuerst (bei Quilt: echte Quilt-Version vor Fabric-Version), dann neueste.
    versions.sort_by(|a, b| {
        let rank = |v: &MrVersion| {
            loaders
                .iter()
                .position(|l| v.loaders.contains(l))
                .unwrap_or(usize::MAX)
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.date_published.cmp(&a.date_published))
    });
    // Neueste erlaubte Version (sortiert: bevorzugter Loader, dann Datum)
    let first_allowed = versions
        .iter()
        .find(|v| target.channel_allowed(&v.version_type))
        .cloned();
    match first_allowed {
        Some(v) => Pick::Found(v),
        None => {
            let mut chans: Vec<String> = versions.iter().map(|v| v.version_type.clone()).collect();
            chans.sort();
            chans.dedup();
            Pick::OnlyChannels(chans)
        }
    }
}

pub fn to_candidate(v: &MrVersion, project: Option<&MrProject>) -> Option<Candidate> {
    let file = v.primary_file()?;
    Some(Candidate {
        source: Source::Modrinth,
        project_id: v.project_id.clone(),
        project_name: project
            .map(|p| p.title.clone())
            .unwrap_or_else(|| v.project_id.clone()),
        project_url: project
            .map(|p| p.url())
            .unwrap_or_else(|| format!("https://modrinth.com/project/{}", v.project_id)),
        icon_url: project.and_then(|p| p.icon_url.clone()),
        version_id: v.id.clone(),
        version_number: v.version_number.clone(),
        channel: v.version_type.clone(),
        file_name: file.filename.clone(),
        download_url: Some(file.url.clone()),
        sha1: file.hashes.get("sha1").cloned(),
        size: file.size,
        game_versions: v.game_versions.clone(),
        loaders: v.loaders.clone(),
        published: v.date_published.clone(),
    })
}
