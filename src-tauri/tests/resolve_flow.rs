//! Ende-zu-Ende-Test der Suchlogik gegen nachgebaute Modrinth-/CurseForge-APIs.

use modsync_lib::curseforge::CurseForge;
use modsync_lib::modrinth::Modrinth;
use modsync_lib::models::*;
use modsync_lib::resolve::{check, Clients};
use serde_json::json;
use std::sync::atomic::AtomicBool;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn local(name: &str, sha: &str, fp: u32, mod_id: Option<&str>) -> LocalMod {
    LocalMod {
        file_name: format!("{name}.jar"),
        path: format!("/mods/{name}.jar"),
        size: 1,
        sha1: format!("{sha}-sha1"),
        sha512: format!("{sha}-sha512"),
        cf_fingerprint: fp,
        meta: mod_id.map(|id| ModMeta {
            loader: "fabric".into(),
            mod_id: id.into(),
            name: Some(name.into()),
            version: Some("1.0".into()),
            minecraft: Some("1.21.1".into()),
            sources: None,
        }),
    }
}

fn mr_version(id: &str, project: &str, gv: &str, vt: &str, date: &str, deps: serde_json::Value) -> serde_json::Value {
    json!({
        "id": id, "project_id": project, "version_number": format!("{id}-v"),
        "game_versions": [gv], "loaders": ["fabric"], "version_type": vt,
        "date_published": date,
        "files": [{ "hashes": {"sha1": format!("{id}sha1"), "sha512": "x"},
                    "url": format!("https://cdn.modrinth.com/{id}.jar"),
                    "filename": format!("{id}.jar"), "primary": true, "size": 10 }],
        "dependencies": deps
    })
}

fn target() -> Target {
    Target {
        game_version: "26.2".into(),
        loader: "fabric".into(),
        allow_beta: false,
        allow_alpha: false,
        include_dependencies: true,
        fallback_by_mod_id: true,
    }
}

async fn setup() -> (MockServer, MockServer) {
    let mr = MockServer::start().await;
    let cf = MockServer::start().await;

    // --- Modrinth ---
    Mock::given(method("POST"))
        .and(path("/v2/version_files"))
        .and(body_partial_json(json!({"algorithm": "sha512"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "a-sha512": mr_version("old-a", "P_A", "1.21.1", "release", "2024-01-01T00:00:00Z", json!([])),
            "u-sha512": mr_version("cur-u", "P_U", "26.2", "release", "2026-09-01T00:00:00Z", json!([]))
        })))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "P_A", "slug": "amod", "title": "A Mod", "icon_url": null, "project_type": "mod"},
            {"id": "P_C", "slug": "cmod", "title": "C Mod", "icon_url": null, "project_type": "mod"},
            {"id": "P_D", "slug": "fabric-api", "title": "Fabric API", "icon_url": null, "project_type": "mod"},
            {"id": "P_U", "slug": "umod", "title": "U Mod", "icon_url": null, "project_type": "mod"}
        ])))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/cmod"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id": "P_C", "slug": "cmod", "title": "C Mod", "icon_url": null, "project_type": "mod"}),
        ))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/P_A/version"))
        .and(query_param("loaders", "[\"fabric\"]"))
        .and(query_param("game_versions", "[\"26.2\"]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            mr_version("a-beta", "P_A", "26.2", "beta", "2026-10-01T00:00:00Z", json!([])),
            mr_version("a-rel", "P_A", "26.2", "release", "2026-09-01T00:00:00Z",
                json!([{"project_id": "P_D", "version_id": null, "dependency_type": "required"},
                       {"project_id": "P_X", "version_id": null, "dependency_type": "optional"}])),
            // Falsch gefilterte Version vom Server wird clientseitig verworfen
            mr_version("a-wrong", "P_A", "1.20.1", "release", "2026-10-05T00:00:00Z", json!([]))
        ])))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/P_C/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            mr_version("c-beta", "P_C", "26.2", "beta", "2026-10-01T00:00:00Z", json!([]))
        ])))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/P_D/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            mr_version("d-rel", "P_D", "26.2", "release", "2026-10-01T00:00:00Z", json!([]))
        ])))
        .mount(&mr)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/project/P_U/version"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            mr_version("cur-u", "P_U", "26.2", "release", "2026-09-01T00:00:00Z", json!([]))
        ])))
        .mount(&mr)
        .await;

    // --- CurseForge ---
    Mock::given(method("POST"))
        .and(path("/v1/fingerprints/432"))
        .and(header("x-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {
            "isCacheBuilt": true,
            "exactMatches": [{"id": 100, "file": {
                "id": 1, "modId": 100, "displayName": "B 1.0", "fileName": "b-old.jar",
                "releaseType": 1, "fileDate": "2024-01-01T00:00:00Z", "fileLength": 5,
                "downloadUrl": "https://edge.forgecdn.net/files/1/b-old.jar",
                "gameVersions": ["1.21.1", "Fabric"], "hashes": [], "dependencies": [],
                "fileFingerprint": 4242
            }, "latestFiles": []}],
            "exactFingerprints": [4242]
        }})))
        .mount(&cf)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/mods"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
            {"id": 100, "name": "B Mod", "slug": "bmod",
             "links": {"websiteUrl": "https://www.curseforge.com/minecraft/mc-mods/bmod"},
             "logo": null, "allowModDistribution": false, "classId": 6}
        ]})))
        .mount(&cf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/100/files"))
        .and(query_param("gameVersion", "26.2"))
        .and(query_param("modLoaderType", "4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{
            "id": 2, "modId": 100, "displayName": "B 2.0", "fileName": "b-new.jar",
            "releaseType": 1, "fileDate": "2026-10-01T00:00:00Z", "fileLength": 6,
            "downloadUrl": null,
            "gameVersions": ["26.2", "Fabric"],
            "hashes": [{"value": "ABCDEF", "algo": 1}], "dependencies": [],
            "fileFingerprint": 1
        }], "pagination": {}})))
        .mount(&cf)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/mods/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&cf)
        .await;

    (mr, cf)
}

#[tokio::test]
async fn full_check_flow() {
    let (mr, cf) = setup().await;
    let clients = Clients {
        modrinth: Modrinth::new(mr.uri()),
        curseforge: Some(CurseForge::new(cf.uri(), "test-key")),
    };
    let mods = vec![
        local("A", "a", 1, Some("amod")),
        local("B", "b", 4242, Some("bmod-id")),
        local("C", "c", 3, Some("cmod")),
        local("E", "e", 5, Some("emod")),
        local("U", "u", 6, Some("umod")),
    ];
    // „U“ ist bereits die Zielversion: sha1 muss mit der Kandidaten-Datei übereinstimmen
    let mut mods = mods;
    mods[4].sha1 = "cur-usha1".into();

    let cancel = AtomicBool::new(false);
    let report = check(mods, &target(), &clients, |_| {}, &cancel)
        .await
        .expect("check failed");
    let row = |k: &str| report.rows.iter().find(|r| r.key == k).unwrap_or_else(|| panic!("row {k}"));

    let a = row("a-sha1");
    assert_eq!(a.status, Status::Available, "{}", a.message);
    assert_eq!(a.match_kind, MatchKind::Hash);
    assert_eq!(a.candidate.as_ref().unwrap().version_id, "a-rel");
    assert_eq!(a.name, "A Mod");

    let b = row("b-sha1");
    assert_eq!(b.status, Status::ManualDownload, "{}", b.message);
    let bc = b.candidate.as_ref().unwrap();
    assert_eq!(bc.sha1.as_deref(), Some("abcdef"));
    assert!(bc.project_url.ends_with("/bmod/files/2"));

    let c = row("c-sha1");
    assert_eq!(c.status, Status::NotAvailable);
    assert_eq!(c.match_kind, MatchKind::ModId);
    assert!(c.message.contains("beta"), "{}", c.message);

    let e = row("e-sha1");
    assert_eq!(e.status, Status::NotFound);

    let u = row("cur-usha1");
    assert_eq!(u.status, Status::UpToDate, "{}", u.message);

    let d = row("dep:mr:P_D");
    assert_eq!(d.status, Status::Available);
    assert_eq!(d.match_kind, MatchKind::Dependency);
    assert_eq!(d.required_by, vec!["A Mod".to_string()]);
    // optionale Abhängigkeit wird nicht aufgenommen
    assert!(report.rows.iter().all(|r| r.key != "dep:mr:P_X"));
    assert_eq!(report.rows.len(), 6);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[tokio::test]
async fn beta_allowed_picks_newest_beta() {
    let (mr, cf) = setup().await;
    let clients = Clients {
        modrinth: Modrinth::new(mr.uri()),
        curseforge: Some(CurseForge::new(cf.uri(), "test-key")),
    };
    let mut t = target();
    t.allow_beta = true;
    t.include_dependencies = false;
    let cancel = AtomicBool::new(false);
    let report = check(vec![local("A", "a", 1, None), local("C", "c", 3, Some("cmod"))], &t, &clients, |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(report.rows[0].candidate.as_ref().unwrap().version_id, "a-beta");
    assert_eq!(report.rows[1].status, Status::Available);
    assert_eq!(report.rows.len(), 2);
}

#[tokio::test]
async fn works_without_curseforge_key() {
    let (mr, _cf) = setup().await;
    let clients = Clients { modrinth: Modrinth::new(mr.uri()), curseforge: None };
    let cancel = AtomicBool::new(false);
    let report = check(vec![local("B", "b", 4242, None)], &target(), &clients, |_| {}, &cancel)
        .await
        .unwrap();
    assert_eq!(report.rows[0].status, Status::NotFound);
    assert!(report.rows[0].message.contains("CurseForge wurde nicht geprüft"));
    assert_eq!(report.warnings.len(), 1);
}

#[tokio::test]
async fn modrinth_offline_gives_warning_not_crash() {
    let clients = Clients { modrinth: Modrinth::new("http://127.0.0.1:9"), curseforge: None };
    let cancel = AtomicBool::new(false);
    let report = check(vec![local("A", "a", 1, None)], &target(), &clients, |_| {}, &cancel)
        .await
        .unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("Keine Verbindung zu Modrinth")), "{:?}", report.warnings);
}
