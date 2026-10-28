//! Java-Entwicklungsumgebung (JDK) finden oder von Eclipse Adoptium herunterladen.

use crate::http;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_ADOPTIUM: &str = "https://api.adoptium.net";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Jdk {
    pub home: String,
    pub major: u32,
    pub version: String,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

/// Liest `release` im JDK-Ordner. Nur echte JDKs (mit javac) zählen.
pub fn inspect(home: &Path) -> Option<Jdk> {
    if !home.join("bin").join(exe("javac")).is_file() {
        return None;
    }
    let release = std::fs::read_to_string(home.join("release")).ok()?;
    let version = release.lines().find_map(|l| {
        l.strip_prefix("JAVA_VERSION=").map(|v| v.trim().trim_matches('"').to_string())
    })?;
    let major = parse_major(&version)?;
    Some(Jdk { home: home.to_string_lossy().to_string(), major, version })
}

pub fn parse_major(v: &str) -> Option<u32> {
    let mut parts = v.split(['.', '_', '+', '-']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

fn candidate_roots(own: &Path) -> Vec<PathBuf> {
    let mut roots = vec![own.to_path_buf()];
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"] {
            if let Ok(base) = std::env::var(var) {
                let base = PathBuf::from(base);
                for vendor in [
                    "Eclipse Adoptium", "Java", "Microsoft", "Zulu", "BellSoft", "Amazon Corretto",
                    "Semeru", "Programs\\Eclipse Adoptium",
                ] {
                    roots.push(base.join(vendor));
                }
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            roots.push(PathBuf::from(&appdata).join("PrismLauncher").join("java"));
        }
        if let Ok(home) = std::env::var("USERPROFILE") {
            roots.push(PathBuf::from(home).join(".jdks"));
        }
    } else {
        roots.push(PathBuf::from("/usr/lib/jvm"));
        if let Ok(home) = std::env::var("HOME") {
            roots.push(PathBuf::from(&home).join(".jdks"));
            roots.push(PathBuf::from(&home).join(".sdkman/candidates/java"));
        }
    }
    roots
}

/// Alle gefundenen JDKs (JAVA_HOME zuerst, dann typische Installationsorte, dann eigene).
pub fn find_all(own_dir: &Path) -> Vec<Jdk> {
    let mut found: Vec<Jdk> = Vec::new();
    let mut push = |j: Option<Jdk>| {
        if let Some(j) = j {
            if !found.iter().any(|f| f.home == j.home) {
                found.push(j);
            }
        }
    };
    if let Ok(jh) = std::env::var("JAVA_HOME") {
        push(inspect(Path::new(&jh)));
    }
    for root in candidate_roots(own_dir) {
        push(inspect(&root));
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                let p = e.path();
                push(inspect(&p));
                // macOS-Layout bzw. verschachtelte Ordner
                push(inspect(&p.join("Contents").join("Home")));
            }
        }
    }
    found
}

/// Bestes JDK: genau die Version, sonst die nächsthöhere.
pub fn pick(all: &[Jdk], required: u32) -> Option<Jdk> {
    all.iter()
        .filter(|j| j.major == required)
        .max_by(|a, b| a.version.cmp(&b.version))
        .or_else(|| all.iter().filter(|j| j.major > required).min_by_key(|j| j.major))
        .cloned()
}

#[derive(Deserialize)]
struct Asset {
    binary: Binary,
    release_name: String,
}
#[derive(Deserialize)]
struct Binary {
    package: Package,
}
#[derive(Deserialize)]
struct Package {
    link: String,
    checksum: Option<String>,
    name: String,
    size: Option<u64>,
}

fn platform() -> (&'static str, &'static str) {
    let os = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "mac" } else { "linux" };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x64" };
    (os, arch)
}

/// Lädt das aktuelle Temurin-JDK und entpackt es in `own_dir`.
pub async fn download<P: Fn(u64, u64)>(
    api_base: &str,
    major: u32,
    own_dir: &Path,
    progress: P,
    cancel: &AtomicBool,
) -> Result<Jdk, String> {
    let (os, arch) = platform();
    let client = http::client();
    let url = format!(
        "{}/v3/assets/latest/{major}/hotspot?architecture={arch}&image_type=jdk&os={os}&vendor=eclipse",
        api_base.trim_end_matches('/')
    );
    let assets: Vec<Asset> = client
        .get(url)
        .send()
        .await
        .map_err(|e| http::describe("Adoptium", &e))?
        .json()
        .await
        .map_err(|e| http::describe("Adoptium", &e))?;
    let asset = assets
        .into_iter()
        .next()
        .ok_or_else(|| format!("Kein JDK {major} für dieses System gefunden."))?;
    let pkg = asset.binary.package;
    if !pkg.link.starts_with("https://") && !cfg!(test) {
        return Err("Unsichere Download-Adresse.".into());
    }

    let resp = http::download_client()
        .get(&pkg.link)
        .send()
        .await
        .map_err(|e| http::describe("Adoptium", &e))?;
    if !resp.status().is_success() {
        return Err(http::describe_status("Adoptium", resp.status()));
    }
    let total = pkg.size.or(resp.content_length()).unwrap_or(0);
    let mut data = Vec::with_capacity(total as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            return Err("Abgebrochen.".into());
        }
        let chunk = chunk.map_err(|e| http::describe("Adoptium", &e))?;
        data.extend_from_slice(&chunk);
        progress(data.len() as u64, total);
    }
    if let Some(sum) = &pkg.checksum {
        let actual = hex::encode(Sha256::digest(&data));
        if !actual.eq_ignore_ascii_case(sum) {
            return Err("Prüfsumme des JDK stimmt nicht – Download verworfen.".into());
        }
    }

    let safe_name: String = asset
        .release_name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.+".contains(c) { c } else { '_' })
        .collect();
    let dest = own_dir.join(&safe_name);
    let tmp = own_dir.join(format!("{safe_name}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    let name = pkg.name.to_lowercase();
    let result = if name.ends_with(".zip") {
        crate::sources::extract_zip(&data, &tmp, true)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(&data, &tmp)
    } else {
        Err(format!("Unbekanntes Archivformat: {}", pkg.name))
    };
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&tmp, &dest).map_err(|e| format!("JDK kann nicht abgelegt werden: {e}"))?;
    let home = if dest.join("Contents/Home").is_dir() { dest.join("Contents/Home") } else { dest };
    inspect(&home).ok_or_else(|| "Heruntergeladenes JDK ist unvollständig.".into())
}

fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<(), String> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut ar = tar::Archive::new(gz);
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in ar.entries().map_err(|e| format!("Archivfehler: {e}"))? {
        let mut entry = entry.map_err(|e| format!("Archivfehler: {e}"))?;
        let path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty()
            || stripped.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            continue;
        }
        let ty = entry.header().entry_type();
        if ty.is_symlink() || ty.is_hard_link() {
            // Symlinks im JDK (legal/) werden nicht benötigt
            continue;
        }
        let out = dest.join(&stripped);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        entry.unpack(&out).map_err(|e| format!("Archivfehler: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jdk(dir: &Path, version: &str) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join(exe("javac")), "").unwrap();
        std::fs::write(dir.join("release"), format!("IMPLEMENTOR=\"x\"\nJAVA_VERSION=\"{version}\"\n")).unwrap();
    }

    #[test]
    fn finds_and_picks_jdks() {
        let d = tempfile::tempdir().unwrap();
        fake_jdk(&d.path().join("jdk-21.0.4"), "21.0.4");
        fake_jdk(&d.path().join("jdk-25.0.1"), "25.0.1");
        fake_jdk(&d.path().join("jdk-26"), "26");
        // JRE ohne javac zählt nicht
        std::fs::create_dir_all(d.path().join("jre-25/bin")).unwrap();
        std::fs::write(d.path().join("jre-25/release"), "JAVA_VERSION=\"25.0.2\"").unwrap();
        let all = find_all(d.path());
        let mine: Vec<_> = all.iter().filter(|j| j.home.starts_with(d.path().to_str().unwrap())).cloned().collect();
        assert_eq!(mine.len(), 3);
        assert_eq!(pick(&mine, 25).unwrap().version, "25.0.1");
        assert_eq!(pick(&mine, 22).unwrap().major, 25);
        assert!(pick(&mine, 27).is_none());
        assert_eq!(parse_major("1.8.0_402"), Some(8));
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        use flate2::{write::GzEncoder, Compression};
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        // tar.gz mit jdk-25/bin/javac + release
        let mut tarbuf = Vec::new();
        {
            let gz = GzEncoder::new(&mut tarbuf, Compression::fast());
            let mut b = tar::Builder::new(gz);
            for (p, c) in [("jdk-25.0.1+8/bin/javac", ""), ("jdk-25.0.1+8/release", "JAVA_VERSION=\"25.0.1\"\n")] {
                let mut h = tar::Header::new_gnu();
                h.set_size(c.len() as u64);
                h.set_mode(0o755);
                h.set_cksum();
                b.append_data(&mut h, p, c.as_bytes()).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let server = MockServer::start().await;
        let sum = hex::encode(Sha256::digest(&tarbuf));
        Mock::given(path("/v3/assets/latest/25/hotspot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "release_name": "jdk-25.0.1+8",
                "binary": {"package": {"link": format!("{}/file.tar.gz", server.uri()), "checksum": sum, "name": "OpenJDK25.tar.gz", "size": tarbuf.len()}}
            }])))
            .mount(&server)
            .await;
        Mock::given(path("/file.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tarbuf.clone()))
            .mount(&server)
            .await;
        let d = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        if cfg!(windows) {
            return; // Test-Archiv ist tar.gz, Windows erwartet ZIP-Namen – Logik identisch
        }
        let jdk = download(&server.uri(), 25, d.path(), |_, _| {}, &cancel).await.unwrap();
        assert_eq!(jdk.major, 25);
        assert!(jdk.home.ends_with("jdk-25.0.1+8"));
    }
}
