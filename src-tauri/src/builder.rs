//! Ablauf „Selbst bauen“: Arbeitskopie anlegen, Vorlage holen, anpassen, bauen.

use crate::patcher::{self, PatchReport, ProjectInfo};
use crate::sources::{self, GithubRepo};
use crate::template::{self, Template};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum BuildSource {
    Github { url: String, branch: String },
    Local { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareRequest {
    pub source: BuildSource,
    pub target_version: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub id: String,
    pub work_dir: String,
    pub project_dir: String,
    pub source_label: String,
    pub template: Template,
    pub info: ProjectInfo,
    pub report: PatchReport,
    pub yarn: Option<String>,
}

pub struct Endpoints {
    pub template: String,
    pub meta: String,
    pub github_api: String,
    pub codeload: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Self {
            template: env("MODSYNC_TEMPLATE_BASE", template::DEFAULT_BASE),
            meta: env("MODSYNC_FABRIC_META_BASE", template::DEFAULT_META),
            github_api: env("MODSYNC_GITHUB_API", "https://api.github.com"),
            codeload: env("MODSYNC_GITHUB_CODELOAD", "https://codeload.github.com"),
        }
    }
}

pub fn slug(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() { "mod".into() } else { s }
}

/// Behält nur die neuesten `keep` Arbeitsordner.
pub fn prune(root: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| e.file_name() != "jdks")
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), e.path()))
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.0));
    for (_, d) in dirs.into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(d);
    }
}

pub async fn prepare(
    req: &PrepareRequest,
    builds_root: &Path,
    ep: &Endpoints,
    cancel: &AtomicBool,
) -> Result<Prepared, String> {
    let target = req.target_version.trim();
    if target.is_empty() {
        return Err("Keine Zielversion angegeben.".into());
    }
    // Vorlage zuerst – schlägt am häufigsten fehl, ohne dass etwas heruntergeladen wurde
    let tpl = template::fetch(&ep.template, target).await?;

    std::fs::create_dir_all(builds_root).map_err(|e| format!("Build-Ordner: {e}"))?;
    prune(builds_root, 12);
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut id = format!("{}-{}-{ts}", slug(&req.name), slug(target));
    let mut n = 1;
    while builds_root.join(&id).exists() {
        n += 1;
        id = format!("{}-{}-{ts}-{n}", slug(&req.name), slug(target));
    }
    let work = builds_root.join(&id);
    let project = work.join("src");

    let source_label = match &req.source {
        BuildSource::Github { url, branch } => {
            let repo: GithubRepo = sources::parse_github(url)
                .ok_or("Nur GitHub-Links werden unterstützt (https://github.com/besitzer/repo).")?;
            if branch.trim().is_empty() {
                return Err("Kein Branch ausgewählt.".into());
            }
            sources::download_github(&ep.codeload, &repo, branch.trim(), &project, cancel).await?;
            format!("github.com/{}/{} @ {}", repo.owner, repo.repo, branch.trim())
        }
        BuildSource::Local { path } => {
            let src = PathBuf::from(path);
            let p2 = project.clone();
            tokio::task::spawn_blocking(move || sources::copy_local(&src, &p2))
                .await
                .map_err(|e| e.to_string())??;
            format!("Lokaler Ordner {path}")
        }
    };

    let info = patcher::analyze(&project)?;
    let yarn = if !tpl.unobfuscated() && info.mappings == patcher::Mappings::Yarn {
        template::latest_yarn(&ep.meta, target).await.ok().flatten()
    } else {
        None
    };
    let report = patcher::apply(&project, &info, &tpl, yarn.as_deref())?;

    let prepared = Prepared {
        id,
        work_dir: work.to_string_lossy().to_string(),
        project_dir: project.to_string_lossy().to_string(),
        source_label,
        template: tpl,
        info,
        report,
        yarn,
    };
    let _ = std::fs::write(
        work.join("plan.json"),
        serde_json::to_string_pretty(&prepared).unwrap_or_default(),
    );
    Ok(prepared)
}

/// Kopiert die gebaute JAR in den Ausgabeordner mit eindeutigem Namen.
pub fn collect_output(jar: &Path, work_dir: &Path, target: &str) -> Result<PathBuf, String> {
    let out = work_dir.join("output");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let stem = jar.file_stem().unwrap_or_default().to_string_lossy();
    let name = format!("{stem}+mc{}-modsync.jar", slug(target).replace('-', "."));
    let dest = out.join(name);
    std::fs::copy(jar, &dest).map_err(|e| format!("JAR kann nicht kopiert werden: {e}"))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Xaero's Minimap!"), "xaero-s-minimap");
        assert_eq!(slug("26.2"), "26-2");
        assert_eq!(slug("///"), "mod");
    }

    #[tokio::test]
    async fn prepare_local_project_end_to_end() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let fx = |f: &str| {
            std::fs::read_to_string(format!("{}/tests/fixtures/template-26.2/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap()
        };
        let server = MockServer::start().await;
        for (route, file) in [
            ("/26.2/gradle.properties", "gradle.properties"),
            ("/26.2/build.gradle", "build.gradle"),
            ("/26.2/gradle/wrapper/gradle-wrapper.properties", "gradle-wrapper.properties"),
            ("/26.2/src/main/resources/fabric.mod.json", "fabric.mod.json"),
        ] {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_string(fx(file)))
                .mount(&server)
                .await;
        }
        let src = tempfile::tempdir().unwrap();
        let s = src.path();
        std::fs::write(s.join("build.gradle"), "plugins { id 'fabric-loom' version \"${loom_version}\" }\ndependencies {\n  mappings loom.officialMojangMappings()\n  modImplementation 'x:y:1'\n}\n").unwrap();
        std::fs::write(s.join("gradle.properties"), "minecraft_version=26.1\nloom_version=1.15-SNAPSHOT\n").unwrap();
        std::fs::write(s.join("gradlew"), "").unwrap();
        std::fs::write(s.join("gradlew.bat"), "").unwrap();
        std::fs::create_dir_all(s.join("build/libs")).unwrap();
        std::fs::write(s.join("build/libs/old.jar"), "").unwrap();

        let builds = tempfile::tempdir().unwrap();
        let ep = Endpoints {
            template: server.uri(),
            meta: server.uri(),
            github_api: server.uri(),
            codeload: server.uri(),
        };
        let req = PrepareRequest {
            source: BuildSource::Local { path: s.to_string_lossy().to_string() },
            target_version: "26.2".into(),
            name: "Test Mod".into(),
        };
        let p = prepare(&req, builds.path(), &ep, &AtomicBool::new(false)).await.unwrap();
        assert!(p.report.blockers.is_empty(), "{:?}", p.report.blockers);
        assert!(p.id.starts_with("test-mod-26-2-"));
        let proj = PathBuf::from(&p.project_dir);
        assert!(!proj.join("build").exists(), "build-Ordner wird nicht mitkopiert");
        let props = std::fs::read_to_string(proj.join("gradle.properties")).unwrap();
        assert!(props.contains("minecraft_version=26.2"));
        // Original unverändert
        assert!(std::fs::read_to_string(s.join("gradle.properties")).unwrap().contains("26.1"));
        assert!(Path::new(&p.work_dir).join("plan.json").exists());

        // Unbekannte Zielversion
        let mut bad = req.clone();
        bad.target_version = "99.9".into();
        let e = prepare(&bad, builds.path(), &ep, &AtomicBool::new(false)).await.unwrap_err();
        assert!(e.contains("keine offizielle Fabric-Vorlage"), "{e}");
    }
}
