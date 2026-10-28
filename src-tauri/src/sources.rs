//! Quellcode besorgen: GitHub-Repository (als ZIP) oder lokaler Ordner – immer als Arbeitskopie.

use crate::http;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_ZIP: u64 = 300 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    pub owner: String,
    pub repo: String,
    pub branch: Option<String>,
}

/// Erkennt `https://github.com/owner/repo(.git)(/tree/branch)`.
pub fn parse_github(url: &str) -> Option<GithubRepo> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let parts: Vec<&str> = u.path_segments()?.filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let owner = parts[0].to_string();
    let repo = parts[1].trim_end_matches(".git").to_string();
    if !valid(&owner) || !valid(&repo) || repo == "." || repo == ".." {
        return None;
    }
    let branch = if parts.len() >= 4 && parts[2] == "tree" {
        Some(parts[3..].join("/"))
    } else {
        None
    };
    Some(GithubRepo { owner, repo, branch })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub default_branch: String,
    pub branches: Vec<String>,
    pub suggested: String,
}

/// Sucht den Branch, der am besten zur Zielversion passt.
pub fn suggest_branch(branches: &[String], default: &str, target: &str) -> String {
    let parts: Vec<&str> = target.split('.').collect();
    let family = if parts.len() >= 2 { format!("{}.{}", parts[0], parts[1]) } else { target.to_string() };
    let score = |b: &str| -> i32 {
        let l = b.to_lowercase();
        let tail = l.rsplit('/').next().unwrap_or(&l);
        let tail = tail.trim_start_matches("mc").trim_start_matches('-');
        if tail == target {
            100
        } else if l.find(target).is_some_and(|i| {
            let rest = &l[i + target.len()..];
            let mut ch = rest.chars();
            match ch.next() {
                Some(c) if c.is_ascii_digit() => false,
                Some('.') => !ch.next().is_some_and(|c| c.is_ascii_digit()),
                _ => true,
            }
        }) {
            80
        } else if tail == format!("{family}.x") || tail == family {
            60
        } else if b == default {
            10
        } else {
            0
        }
    };
    branches
        .iter()
        .max_by_key(|b| (score(b), *b == default))
        .filter(|b| score(b) > 0)
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

#[derive(Deserialize)]
struct RepoMeta {
    default_branch: String,
}

#[derive(Deserialize)]
struct BranchMeta {
    name: String,
}

pub async fn branches(api_base: &str, repo: &GithubRepo, target: &str) -> Result<BranchInfo, String> {
    let client = http::client();
    let base = format!("{}/repos/{}/{}", api_base.trim_end_matches('/'), repo.owner, repo.repo);
    let resp = client
        .get(&base)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| http::describe("GitHub", &e))?;
    match resp.status().as_u16() {
        404 => return Err(format!("Repository {}/{} nicht gefunden (oder privat).", repo.owner, repo.repo)),
        403 | 429 => return Err("GitHub-Anfragelimit erreicht – in einer Stunde erneut versuchen.".into()),
        s if !(200..300).contains(&s) => return Err(http::describe_status("GitHub", resp.status())),
        _ => {}
    }
    let meta: RepoMeta = resp.json().await.map_err(|e| http::describe("GitHub", &e))?;
    let list: Vec<BranchMeta> = client
        .get(format!("{base}/branches?per_page=100"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| http::describe("GitHub", &e))?
        .json()
        .await
        .unwrap_or_default();
    let mut names: Vec<String> = list.into_iter().map(|b| b.name).collect();
    if !names.contains(&meta.default_branch) {
        names.insert(0, meta.default_branch.clone());
    }
    let suggested = repo
        .branch
        .clone()
        .filter(|b| names.contains(b))
        .unwrap_or_else(|| suggest_branch(&names, &meta.default_branch, target));
    Ok(BranchInfo { default_branch: meta.default_branch, branches: names, suggested })
}

/// Lädt einen Branch als ZIP und entpackt ihn (ohne Wurzelordner) nach `dest`.
pub async fn download_github(
    codeload_base: &str,
    repo: &GithubRepo,
    branch: &str,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let enc: String = branch
        .split('/')
        .map(|p| url::form_urlencoded::byte_serialize(p.as_bytes()).collect::<String>())
        .collect::<Vec<_>>()
        .join("/");
    let url = format!(
        "{}/{}/{}/zip/refs/heads/{}",
        codeload_base.trim_end_matches('/'),
        repo.owner,
        repo.repo,
        enc
    );
    let resp = http::download_client()
        .get(url)
        .send()
        .await
        .map_err(|e| http::describe("GitHub", &e))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("Branch „{branch}“ nicht gefunden."));
    }
    if !resp.status().is_success() {
        return Err(http::describe_status("GitHub", resp.status()));
    }
    let mut data = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            return Err("Abgebrochen.".into());
        }
        let chunk = chunk.map_err(|e| http::describe("GitHub", &e))?;
        data.extend_from_slice(&chunk);
        if data.len() as u64 > MAX_ZIP {
            return Err("Das Repository ist zu groß (über 300 MB).".into());
        }
    }
    extract_zip(&data, dest, true)
}

/// Entpackt sicher: keine Pfade außerhalb von `dest`, optional ohne obersten Ordner.
pub fn extract_zip(data: &[u8], dest: &Path, strip_root: bool) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| format!("Ungültiges ZIP: {e}"))?;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("ZIP-Fehler: {e}"))?;
        let Some(path) = f.enclosed_name() else { continue };
        let path: PathBuf = if strip_root {
            let mut comps = path.components();
            comps.next();
            comps.as_path().to_path_buf()
        } else {
            path
        };
        if path.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&path);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        total += f.size();
        if total > 2 * MAX_ZIP {
            return Err("Entpackter Inhalt ist zu groß.".into());
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf).map_err(|e| format!("ZIP-Fehler: {e}"))?;
        std::fs::write(&out, buf).map_err(|e| format!("{}: {e}", out.display()))?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode & 0o755));
        }
    }
    Ok(())
}

/// Kopiert einen lokalen Projektordner (ohne build/.gradle/.git) in die Arbeitskopie.
pub fn copy_local(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("Ordner nicht gefunden: {}", src.display()));
    }
    fn rec(src: &Path, dest: &Path, depth: usize) -> std::io::Result<()> {
        if depth > 30 {
            return Ok(());
        }
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let name = e.file_name();
            let n = name.to_string_lossy();
            let ft = e.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if matches!(n.as_ref(), ".git" | ".gradle" | "build" | "run" | "out" | ".idea" | "node_modules") {
                    continue;
                }
                rec(&e.path(), &dest.join(&name), depth + 1)?;
            } else {
                std::fs::copy(e.path(), dest.join(&name))?;
            }
        }
        Ok(())
    }
    rec(src, dest, 0).map_err(|e| format!("Kopieren fehlgeschlagen: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_urls() {
        assert_eq!(
            parse_github("https://github.com/CaffeineMC/sodium"),
            Some(GithubRepo { owner: "CaffeineMC".into(), repo: "sodium".into(), branch: None })
        );
        assert_eq!(
            parse_github("https://github.com/a/b.git").unwrap().repo,
            "b"
        );
        assert_eq!(
            parse_github("https://github.com/a/b/tree/1.21/dev").unwrap().branch.as_deref(),
            Some("1.21/dev")
        );
        assert!(parse_github("https://gitlab.com/a/b").is_none());
        assert!(parse_github("https://github.com/a").is_none());
        assert!(parse_github("https://github.com/a/..").is_none());
    }

    #[test]
    fn suggests_branches() {
        let b = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(suggest_branch(&b(&["main", "1.21.1", "26.2"]), "main", "26.2"), "26.2");
        assert_eq!(suggest_branch(&b(&["main", "mc26.2", "1.21"]), "main", "26.2"), "mc26.2");
        assert_eq!(suggest_branch(&b(&["main", "1.21.x", "1.20.x"]), "main", "1.21.4"), "1.21.x");
        assert_eq!(suggest_branch(&b(&["dev", "1.21.1"]), "dev", "26.2"), "dev");
        assert_eq!(suggest_branch(&b(&["main", "26.2.1"]), "main", "26.2"), "main");
    }

    #[test]
    fn zip_slip_is_blocked() {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("repo-main/build.gradle", o).unwrap();
            z.write_all(b"ok").unwrap();
            z.start_file("../../evil.txt", o).unwrap();
            z.write_all(b"bad").unwrap();
            z.finish().unwrap();
        }
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("src");
        extract_zip(buf.get_ref(), &dest, true).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("build.gradle")).unwrap(), "ok");
        assert!(!d.path().join("evil.txt").exists());
    }
}
