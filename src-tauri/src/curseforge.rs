use crate::http;
use crate::models::{Candidate, Source, Target};
use serde::Deserialize;
use std::collections::HashMap;

pub const DEFAULT_BASE: &str = "https://api.curseforge.com";
pub const MINECRAFT_GAME_ID: u32 = 432;
pub const MODS_CLASS_ID: u32 = 6;
const SERVICE: &str = "CurseForge";

#[derive(Debug, Deserialize)]
struct Wrapped<T> {
    data: T,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfHash {
    pub value: String,
    pub algo: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfDependency {
    pub mod_id: u64,
    pub relation_type: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfFile {
    pub id: u64,
    pub mod_id: u64,
    #[serde(default)]
    pub display_name: String,
    pub file_name: String,
    pub release_type: u32,
    #[serde(default)]
    pub file_date: String,
    #[serde(default)]
    pub file_length: u64,
    pub download_url: Option<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub hashes: Vec<CfHash>,
    #[serde(default)]
    pub dependencies: Vec<CfDependency>,
    #[serde(default = "yes")]
    pub is_available: bool,
    #[serde(default)]
    pub file_fingerprint: u32,
}

fn yes() -> bool {
    true
}

impl CfFile {
    pub fn channel(&self) -> &'static str {
        match self.release_type {
            1 => "release",
            2 => "beta",
            3 => "alpha",
            _ => "unknown",
        }
    }

    pub fn sha1(&self) -> Option<String> {
        self.hashes
            .iter()
            .find(|h| h.algo == 1)
            .map(|h| h.value.to_lowercase())
    }

    /// relationType 3 = RequiredDependency
    pub fn required_dependencies(&self) -> Vec<u64> {
        self.dependencies
            .iter()
            .filter(|d| d.relation_type == 3)
            .map(|d| d.mod_id)
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CfLinks {
    pub website_url: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfLogo {
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CfMod {
    pub id: u64,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub links: CfLinks,
    pub logo: Option<CfLogo>,
    pub allow_mod_distribution: Option<bool>,
    pub class_id: Option<u32>,
}

impl CfMod {
    pub fn url(&self) -> String {
        self.links
            .website_url
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("https://www.curseforge.com/minecraft/mc-mods/{}", self.slug))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintMatch {
    id: u64,
    file: CfFile,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintResult {
    #[serde(default)]
    exact_matches: Vec<FingerprintMatch>,
}

#[derive(Clone)]
pub struct CurseForge {
    http: reqwest::Client,
    base: String,
    key: String,
}

impl CurseForge {
    pub fn new(base: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            http: http::client(),
            base: base.into().trim_end_matches('/').to_string(),
            key: key.into(),
        }
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<Option<T>, String> {
        let resp = req
            .header("x-api-key", &self.key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| http::describe(SERVICE, &e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(http::describe_status(SERVICE, status));
        }
        resp.json::<Wrapped<T>>()
            .await
            .map(|w| Some(w.data))
            .map_err(|e| http::describe(SERVICE, &e))
    }

    /// Prüft, ob der API-Key akzeptiert wird.
    pub async fn test_key(&self) -> Result<(), String> {
        let req = self
            .http
            .get(format!("{}/v1/games/{}", self.base, MINECRAFT_GAME_ID));
        self.send::<serde_json::Value>(req).await?;
        Ok(())
    }

    /// Fingerprint → Mod-ID
    pub async fn identify(&self, fingerprints: &[u32]) -> Result<HashMap<u32, u64>, String> {
        let mut out = HashMap::new();
        for chunk in fingerprints.chunks(200) {
            let req = self
                .http
                .post(format!(
                    "{}/v1/fingerprints/{}",
                    self.base, MINECRAFT_GAME_ID
                ))
                .json(&serde_json::json!({ "fingerprints": chunk }));
            if let Some(res) = self.send::<FingerprintResult>(req).await? {
                for m in res.exact_matches {
                    out.insert(m.file.file_fingerprint, m.id);
                }
            }
        }
        Ok(out)
    }

    pub async fn mods(&self, ids: &[u64]) -> Result<HashMap<u64, CfMod>, String> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(100) {
            let req = self
                .http
                .post(format!("{}/v1/mods", self.base))
                .json(&serde_json::json!({ "modIds": chunk, "filterPcOnly": true }));
            if let Some(list) = self.send::<Vec<CfMod>>(req).await? {
                for m in list {
                    out.insert(m.id, m);
                }
            }
        }
        Ok(out)
    }

    /// Suche nach exaktem Slug (Fallback über die Mod-ID).
    pub async fn mod_by_slug(&self, slug: &str) -> Result<Option<CfMod>, String> {
        let req = self
            .http
            .get(format!("{}/v1/mods/search", self.base))
            .query(&[
                ("gameId", MINECRAFT_GAME_ID.to_string()),
                ("classId", MODS_CLASS_ID.to_string()),
                ("slug", slug.to_string()),
            ]);
        Ok(self
            .send::<Vec<CfMod>>(req)
            .await?
            .unwrap_or_default()
            .into_iter()
            .find(|m| m.slug.eq_ignore_ascii_case(slug)))
    }

    pub async fn files(&self, mod_id: u64, target: &Target) -> Result<Vec<CfFile>, String> {
        let mut query = vec![
            ("gameVersion", target.game_version.clone()),
            ("pageSize", "50".to_string()),
        ];
        if let Some(t) = target.curseforge_loader_type() {
            query.push(("modLoaderType", t.to_string()));
        }
        let req = self
            .http
            .get(format!("{}/v1/mods/{}/files", self.base, mod_id))
            .query(&query);
        Ok(self.send::<Vec<CfFile>>(req).await?.unwrap_or_default())
    }
}

pub enum Pick {
    Found(CfFile),
    OnlyChannels(Vec<String>),
    None,
}

pub fn pick_file(mut files: Vec<CfFile>, target: &Target) -> Pick {
    let loaders: Vec<String> = target
        .curseforge_loaders()
        .iter()
        .map(|l| l.to_lowercase())
        .collect();
    files.retain(|f| {
        f.is_available
            && f.file_name.to_lowercase().ends_with(".jar")
            && f.game_versions.iter().any(|g| g == &target.game_version)
            && f
                .game_versions
                .iter()
                .any(|g| loaders.contains(&g.to_lowercase()))
    });
    if files.is_empty() {
        return Pick::None;
    }
    files.sort_by(|a, b| {
        let rank = |f: &CfFile| {
            loaders
                .iter()
                .position(|l| f.game_versions.iter().any(|g| &g.to_lowercase() == l))
                .unwrap_or(usize::MAX)
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.file_date.cmp(&a.file_date))
    });
    match files.iter().find(|f| target.channel_allowed(f.channel())) {
        Some(f) => Pick::Found(f.clone()),
        None => {
            let mut chans: Vec<String> = files.iter().map(|f| f.channel().to_string()).collect();
            chans.sort();
            chans.dedup();
            Pick::OnlyChannels(chans)
        }
    }
}

pub fn to_candidate(f: &CfFile, m: Option<&CfMod>) -> Candidate {
    let project_url = m
        .map(|m| m.url())
        .unwrap_or_else(|| format!("https://www.curseforge.com/projects/{}", f.mod_id));
    // allowModDistribution == false → kein direkter Download über Drittprogramme
    let blocked = m.and_then(|m| m.allow_mod_distribution) == Some(false);
    let download_url = if blocked { None } else { f.download_url.clone() };
    Candidate {
        source: Source::Curseforge,
        project_id: f.mod_id.to_string(),
        project_name: m.map(|m| m.name.clone()).unwrap_or_else(|| f.display_name.clone()),
        icon_url: m.and_then(|m| m.logo.as_ref()).and_then(|l| l.thumbnail_url.clone()),
        version_id: f.id.to_string(),
        version_number: f.display_name.clone(),
        channel: f.channel().to_string(),
        file_name: f.file_name.clone(),
        download_url,
        sha1: f.sha1(),
        size: f.file_length,
        game_versions: f.game_versions.clone(),
        loaders: Vec::new(),
        published: f.file_date.clone(),
        project_url: format!("{}/files/{}", project_url.trim_end_matches('/'), f.id),
    }
}
