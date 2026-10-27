//! Herunterladen und Installieren in den Zielordner – mit Hash-Prüfung und ohne Überschreiben.

use crate::http;
use crate::models::{InstallItem, InstallResult, Progress};
use futures_util::StreamExt;
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

/// Dateiname darf keine Pfadbestandteile enthalten und muss eine .jar sein.
pub fn safe_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    let bad = trimmed.is_empty()
        || trimmed.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        || trimmed.starts_with('.')
        || trimmed.contains("..")
        || trimmed.chars().any(|c| (c as u32) < 0x20);
    if bad {
        return Err(format!("Ungültiger Dateiname: {name}"));
    }
    if !trimmed.to_lowercase().ends_with(".jar") {
        return Err(format!("Keine .jar-Datei: {name}"));
    }
    Ok(trimmed.to_string())
}

fn safe_url(raw: &str) -> Result<url::Url, String> {
    let u = url::Url::parse(raw).map_err(|_| format!("Ungültige Download-Adresse: {raw}"))?;
    let local_test = cfg!(test) && u.host_str() == Some("127.0.0.1");
    if u.scheme() != "https" && !local_test {
        return Err("Nur HTTPS-Downloads sind erlaubt.".into());
    }
    Ok(u)
}

fn file_sha1(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    Some(hex::encode(Sha1::digest(&data)))
}

pub async fn install_all<P: Fn(Progress) + Send + Sync>(
    items: Vec<InstallItem>,
    target_dir: &Path,
    progress: P,
    cancel: &AtomicBool,
) -> Result<Vec<InstallResult>, String> {
    if !target_dir.is_dir() {
        std::fs::create_dir_all(target_dir).map_err(|e| {
            format!(
                "Zielordner kann nicht angelegt werden ({}): {e}",
                target_dir.display()
            )
        })?;
    }
    let client = http::download_client();
    let total = items.len();
    let mut results = Vec::with_capacity(total);

    for (i, item) in items.into_iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            results.push(InstallResult {
                key: item.key,
                file_name: item.file_name,
                outcome: "cancelled".into(),
                message: "Abgebrochen.".into(),
            });
            continue;
        }
        progress(Progress {
            phase: "install".into(),
            done: i,
            total,
            label: item.file_name.clone(),
        });
        let r = install_one(&client, &item, target_dir, cancel).await;
        results.push(match r {
            Ok((outcome, message)) => InstallResult {
                key: item.key,
                file_name: item.file_name,
                outcome: outcome.into(),
                message,
            },
            Err(e) => InstallResult {
                key: item.key,
                file_name: item.file_name,
                outcome: if cancel.load(Ordering::SeqCst) {
                    "cancelled".into()
                } else {
                    "failed".into()
                },
                message: e,
            },
        });
    }
    progress(Progress {
        phase: "install".into(),
        done: total,
        total,
        label: "Fertig".into(),
    });
    Ok(results)
}

async fn install_one(
    client: &reqwest::Client,
    item: &InstallItem,
    dir: &Path,
    cancel: &AtomicBool,
) -> Result<(&'static str, String), String> {
    let name = safe_file_name(&item.file_name)?;
    let url = safe_url(&item.url)?;
    let dest: PathBuf = dir.join(&name);
    let expected = item.sha1.as_ref().map(|s| s.to_lowercase());

    if dest.exists() {
        let existing = file_sha1(&dest);
        return match (&expected, existing) {
            (Some(e), Some(h)) if *e == h => Ok(("skipped", "Bereits vorhanden.".into())),
            (None, _) => Ok(("skipped", "Datei existiert bereits – nicht überschrieben.".into())),
            _ => Err("Eine andere Datei mit gleichem Namen existiert bereits – nicht überschrieben.".into()),
        };
    }

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| http::describe("Download-Server", &e))?;
    if !resp.status().is_success() {
        return Err(http::describe_status("Download-Server", resp.status()));
    }

    let part = dir.join(format!("{name}.part"));
    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("Datei kann nicht geschrieben werden: {e}"))?;
    let mut hasher = Sha1::new();
    let mut stream = resp.bytes_stream();
    let result: Result<(), String> = async {
        while let Some(chunk) = stream.next().await {
            if cancel.load(Ordering::SeqCst) {
                return Err("Abgebrochen.".into());
            }
            let chunk = chunk.map_err(|e| http::describe("Download-Server", &e))?;
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Schreibfehler: {e}"))?;
        }
        file.flush().await.map_err(|e| format!("Schreibfehler: {e}"))?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    let actual = hex::encode(hasher.finalize());
    if let Some(e) = &expected {
        if *e != actual {
            let _ = tokio::fs::remove_file(&part).await;
            return Err("Prüfsumme stimmt nicht – Datei verworfen.".into());
        }
    }
    tokio::fs::rename(&part, &dest)
        .await
        .map_err(|e| format!("Datei kann nicht umbenannt werden: {e}"))?;
    let msg = if expected.is_some() {
        "Installiert (Prüfsumme OK)."
    } else {
        "Installiert (keine Prüfsumme verfügbar)."
    };
    Ok(("installed", msg.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_names() {
        assert!(safe_file_name("../evil.jar").is_err());
        assert!(safe_file_name("sub/evil.jar").is_err());
        assert!(safe_file_name("C:evil.jar").is_err());
        assert!(safe_file_name("evil.exe").is_err());
        assert!(safe_file_name(".hidden.jar").is_err());
        assert_eq!(safe_file_name("sodium-fabric-0.6.jar").unwrap(), "sodium-fabric-0.6.jar");
    }

    #[tokio::test]
    async fn downloads_verifies_and_never_overwrites() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let body = b"fake jar content".to_vec();
        Mock::given(path("/good.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let good_sha = hex::encode(Sha1::digest(&body));
        let cancel = AtomicBool::new(false);
        let items = vec![
            InstallItem {
                key: "a".into(),
                file_name: "good.jar".into(),
                url: format!("{}/good.jar", server.uri()),
                sha1: Some(good_sha.clone()),
                size: 0,
            },
            InstallItem {
                key: "b".into(),
                file_name: "bad.jar".into(),
                url: format!("{}/good.jar", server.uri()),
                sha1: Some("0".repeat(40)),
                size: 0,
            },
            InstallItem {
                key: "c".into(),
                file_name: "../escape.jar".into(),
                url: format!("{}/good.jar", server.uri()),
                sha1: None,
                size: 0,
            },
        ];
        let res = install_all(items.clone(), dir.path(), |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(res[0].outcome, "installed");
        assert_eq!(res[1].outcome, "failed");
        assert!(!dir.path().join("bad.jar").exists());
        assert!(!dir.path().join("bad.jar.part").exists());
        assert_eq!(res[2].outcome, "failed");
        assert_eq!(std::fs::read(dir.path().join("good.jar")).unwrap(), body);

        // Zweiter Lauf: gleiche Datei wird übersprungen
        let res2 = install_all(items[..1].to_vec(), dir.path(), |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(res2[0].outcome, "skipped");

        // Andere Datei mit gleichem Namen wird nicht überschrieben
        std::fs::write(dir.path().join("other.jar"), b"mine").unwrap();
        let mut other = items[0].clone();
        other.file_name = "other.jar".into();
        let res3 = install_all(vec![other], dir.path(), |_| {}, &cancel)
            .await
            .unwrap();
        assert_eq!(res3[0].outcome, "failed");
        assert_eq!(std::fs::read(dir.path().join("other.jar")).unwrap(), b"mine");
    }

    #[test]
    fn rejects_http() {
        assert!(safe_url("http://cdn.modrinth.com/x.jar").is_err());
        assert!(safe_url("https://cdn.modrinth.com/x.jar").is_ok());
    }
}
