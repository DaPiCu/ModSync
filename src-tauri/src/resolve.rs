//! Kernlogik: Mods identifizieren, passende Versionen für das Ziel suchen, Abhängigkeiten auflösen.

use crate::curseforge::{self, CfMod, CurseForge};
use crate::modrinth::{self, Modrinth, MrProject};
use crate::models::*;
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

const PARALLEL: usize = 6;
const MAX_DEP_ROUNDS: usize = 6;

pub struct Clients {
    pub modrinth: Modrinth,
    pub curseforge: Option<CurseForge>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub rows: Vec<ResultRow>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Ident {
    mr_project: Option<String>,
    cf_mod: Option<u64>,
    match_kind: Option<MatchKind>,
}

/// Ergebnis der Versionssuche für ein Projekt.
#[derive(Debug, Clone)]
struct Resolution {
    candidate: Option<Candidate>,
    mr_deps: Vec<(Option<String>, Option<String>)>,
    cf_deps: Vec<u64>,
    /// Kanäle, in denen es nur Versionen gibt (wenn nichts Erlaubtes gefunden wurde)
    only_channels: Vec<String>,
    errors: Vec<String>,
}

fn norm_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn loader_label(l: &str) -> &str {
    match l {
        "fabric" => "Fabric",
        "quilt" => "Quilt",
        "neoforge" => "NeoForge",
        "forge" => "Forge",
        other => other,
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("Vorgang abgebrochen.".into())
    } else {
        Ok(())
    }
}

async fn resolve_project(
    clients: &Clients,
    target: &Target,
    mr_project: Option<&str>,
    mr_meta: Option<&MrProject>,
    cf_mod: Option<u64>,
    cf_meta: Option<&CfMod>,
) -> Resolution {
    let mut res = Resolution {
        candidate: None,
        mr_deps: vec![],
        cf_deps: vec![],
        only_channels: vec![],
        errors: vec![],
    };

    if let Some(pid) = mr_project {
        match clients.modrinth.project_versions(pid, target).await {
            Ok(versions) => match modrinth::pick_version(versions, target) {
                modrinth::Pick::Found(v) => {
                    res.mr_deps = v.required_dependency_refs();
                    res.candidate = modrinth::to_candidate(&v, mr_meta);
                    return res;
                }
                modrinth::Pick::OnlyChannels(c) => res.only_channels.extend(c),
                modrinth::Pick::None => {}
            },
            Err(e) => res.errors.push(e),
        }
    }

    if let (Some(mid), Some(cf)) = (cf_mod, clients.curseforge.as_ref()) {
        match cf.files(mid, target).await {
            Ok(files) => match curseforge::pick_file(files, target) {
                curseforge::Pick::Found(f) => {
                    res.cf_deps = f.required_dependencies();
                    res.candidate = Some(curseforge::to_candidate(&f, cf_meta));
                    res.only_channels.clear();
                    return res;
                }
                curseforge::Pick::OnlyChannels(c) => res.only_channels.extend(c),
                curseforge::Pick::None => {}
            },
            Err(e) => res.errors.push(e),
        }
    }
    res.only_channels.sort();
    res.only_channels.dedup();
    res
}

fn known_on(
    mr_project: Option<&str>,
    mr_meta: &HashMap<String, MrProject>,
    cf_mod: Option<u64>,
    cf_meta: &HashMap<u64, CfMod>,
) -> Vec<KnownOn> {
    let mut out = vec![];
    if let Some(pid) = mr_project {
        let p = mr_meta.get(pid);
        out.push(KnownOn {
            source: Source::Modrinth,
            project_id: pid.to_string(),
            project_name: p.map(|p| p.title.clone()).unwrap_or_else(|| pid.to_string()),
            url: p
                .map(|p| p.url())
                .unwrap_or_else(|| format!("https://modrinth.com/project/{pid}")),
        });
    }
    if let Some(mid) = cf_mod {
        let m = cf_meta.get(&mid);
        out.push(KnownOn {
            source: Source::Curseforge,
            project_id: mid.to_string(),
            project_name: m.map(|m| m.name.clone()).unwrap_or_else(|| mid.to_string()),
            url: m
                .map(|m| m.url())
                .unwrap_or_else(|| format!("https://www.curseforge.com/projects/{mid}")),
        });
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn build_row(
    key: String,
    name: String,
    local: Option<LocalMod>,
    required_by: Vec<String>,
    match_kind: MatchKind,
    known: Vec<KnownOn>,
    res: &Resolution,
    target: &Target,
    cf_enabled: bool,
) -> ResultRow {
    let tv = format!("{} ({})", target.game_version, loader_label(&target.loader));
    let (status, message) = if let Some(c) = &res.candidate {
        let same_file = match (&c.sha1, &local) {
            (Some(s), Some(l)) => s.eq_ignore_ascii_case(&l.sha1),
            _ => false,
        };
        if same_file {
            (Status::UpToDate, "Die vorhandene Datei passt bereits zur Zielversion.".to_string())
        } else if c.download_url.is_none() {
            (
                Status::ManualDownload,
                "Der Autor erlaubt keine Downloads über Drittprogramme – bitte über die Website herunterladen.".to_string(),
            )
        } else {
            let mut m = format!("{} gefunden", c.version_number);
            if c.channel != "release" {
                m.push_str(&format!(" ({})", c.channel));
            }
            if match_kind == MatchKind::ModId {
                m.push_str(" – Treffer über Mod-ID, bitte prüfen");
            }
            (Status::Available, m)
        }
    } else if known.is_empty() {
        if !res.errors.is_empty() {
            (Status::Error, res.errors.join(" "))
        } else {
            let mut m = "Weder auf Modrinth noch auf CurseForge gefunden (eventuell von einer anderen Seite oder selbst gebaut).".to_string();
            if !cf_enabled {
                m = "Nicht auf Modrinth gefunden. CurseForge wurde nicht geprüft (kein API-Key).".into();
            }
            (Status::NotFound, m)
        }
    } else if !res.errors.is_empty() {
        (Status::Error, res.errors.join(" "))
    } else if !res.only_channels.is_empty() {
        (
            Status::NotAvailable,
            format!(
                "Für {tv} nur als {} verfügbar – in den Optionen freischalten.",
                res.only_channels.join("/")
            ),
        )
    } else {
        (Status::NotAvailable, format!("Noch keine Version für {tv}."))
    };

    ResultRow {
        key,
        name,
        local,
        required_by,
        match_kind,
        status,
        candidate: res.candidate.clone(),
        known_on: known,
        message,
    }
}

pub async fn check<P: Fn(Progress) + Send + Sync>(
    mods: Vec<LocalMod>,
    target: &Target,
    clients: &Clients,
    progress: P,
    cancel: &AtomicBool,
) -> Result<CheckReport, String> {
    if target.game_version.trim().is_empty() {
        return Err("Bitte eine Minecraft-Zielversion auswählen.".into());
    }
    let mut warnings = Vec::new();
    let cf_enabled = clients.curseforge.is_some();
    if !cf_enabled {
        warnings.push(
            "Kein CurseForge-API-Key hinterlegt – es wird nur Modrinth durchsucht.".to_string(),
        );
    }

    // ---- 1. Identifizieren per Hash ------------------------------------------------------
    progress(Progress {
        phase: "identify".into(),
        done: 0,
        total: mods.len(),
        label: "Dateien werden auf Modrinth/CurseForge erkannt …".into(),
    });
    let sha512s: Vec<String> = mods.iter().map(|m| m.sha512.clone()).collect();
    let mr_hits = match clients.modrinth.versions_by_hashes(&sha512s).await {
        Ok(h) => h,
        Err(e) => {
            warnings.push(e);
            HashMap::new()
        }
    };
    let cf_hits = match &clients.curseforge {
        Some(cf) => {
            let fps: Vec<u32> = mods.iter().map(|m| m.cf_fingerprint).collect();
            match cf.identify(&fps).await {
                Ok(h) => h,
                Err(e) => {
                    warnings.push(e);
                    HashMap::new()
                }
            }
        }
        None => HashMap::new(),
    };
    check_cancel(cancel)?;

    let mut idents: Vec<Ident> = mods
        .iter()
        .map(|m| {
            let mr = mr_hits.get(&m.sha512).map(|v| v.project_id.clone());
            let cf = cf_hits.get(&m.cf_fingerprint).copied();
            let kind = if mr.is_some() || cf.is_some() {
                Some(MatchKind::Hash)
            } else {
                None
            };
            Ident {
                mr_project: mr,
                cf_mod: cf,
                match_kind: kind,
            }
        })
        .collect();

    // ---- 2. Fallback über Mod-ID ---------------------------------------------------------
    if target.fallback_by_mod_id {
        let todo: Vec<(usize, String)> = mods
            .iter()
            .enumerate()
            .filter(|(i, _)| idents[*i].match_kind.is_none())
            .filter_map(|(i, m)| m.meta.as_ref().map(|meta| (i, meta.mod_id.clone())))
            .collect();
        let futs: Vec<_> = todo
            .into_iter()
            .map(|(i, mod_id)| async move {
                let mr = match clients.modrinth.project(&mod_id).await {
                    Ok(Some(p)) if p.project_type == "mod" || p.project_type.is_empty() => {
                        Some(p.id)
                    }
                    _ => None,
                };
                let cf = match &clients.curseforge {
                    Some(cf) => cf.mod_by_slug(&mod_id).await.ok().flatten().map(|m| m.id),
                    None => None,
                };
                (i, mr, cf)
            })
            .collect();
        let found: Vec<(usize, Option<String>, Option<u64>)> =
            stream::iter(futs).buffer_unordered(PARALLEL).collect().await;
        for (i, mr, cf) in found {
            if mr.is_some() || cf.is_some() {
                idents[i] = Ident {
                    mr_project: mr,
                    cf_mod: cf,
                    match_kind: Some(MatchKind::ModId),
                };
            }
        }
    }
    check_cancel(cancel)?;

    // ---- 3. Projekt-Metadaten ------------------------------------------------------------
    let mut mr_meta: HashMap<String, MrProject> = HashMap::new();
    let mut cf_meta: HashMap<u64, CfMod> = HashMap::new();
    let mr_ids: Vec<String> = {
        let s: HashSet<String> = idents.iter().filter_map(|i| i.mr_project.clone()).collect();
        s.into_iter().collect()
    };
    if !mr_ids.is_empty() {
        match clients.modrinth.projects(&mr_ids).await {
            Ok(m) => mr_meta.extend(m),
            Err(e) => warnings.push(e),
        }
    }
    let cf_ids: Vec<u64> = {
        let s: HashSet<u64> = idents.iter().filter_map(|i| i.cf_mod).collect();
        s.into_iter().collect()
    };
    if let (false, Some(cf)) = (cf_ids.is_empty(), &clients.curseforge) {
        match cf.mods(&cf_ids).await {
            Ok(m) => cf_meta.extend(m),
            Err(e) => warnings.push(e),
        }
    }

    // ---- 4. Versionen für das Ziel suchen ------------------------------------------------
    let total = mods.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let progress_ref = &progress;
    let mr_meta_ref = &mr_meta;
    let cf_meta_ref = &cf_meta;
    let futs: Vec<_> = idents
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let done = &done;
            let name = mods[i].display_name();
            async move {
                let res = if cancel.load(Ordering::SeqCst) || id.match_kind.is_none() {
                    Resolution {
                        candidate: None,
                        mr_deps: vec![],
                        cf_deps: vec![],
                        only_channels: vec![],
                        errors: vec![],
                    }
                } else {
                    resolve_project(
                        clients,
                        target,
                        id.mr_project.as_deref(),
                        id.mr_project.as_ref().and_then(|p| mr_meta_ref.get(p)),
                        id.cf_mod,
                        id.cf_mod.and_then(|m| cf_meta_ref.get(&m)),
                    )
                    .await
                };
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                progress_ref(Progress {
                    phase: "versions".into(),
                    done: n,
                    total,
                    label: name,
                });
                (i, res)
            }
        })
        .collect();
    let mut resolved: Vec<(usize, Resolution)> =
        stream::iter(futs).buffer_unordered(PARALLEL).collect().await;
    check_cancel(cancel)?;
    resolved.sort_by_key(|(i, _)| *i);

    let mut rows = Vec::new();
    let mut seen_mr: HashSet<String> = HashSet::new();
    let mut seen_cf: HashSet<u64> = HashSet::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut pending_mr: Vec<(Option<String>, Option<String>, String)> = vec![];
    let mut pending_cf: Vec<(u64, String)> = vec![];

    for (i, res) in &resolved {
        let m = &mods[*i];
        let id = &idents[*i];
        if let Some(p) = &id.mr_project {
            seen_mr.insert(p.clone());
        }
        if let Some(c) = id.cf_mod {
            seen_cf.insert(c);
        }
        let known = known_on(id.mr_project.as_deref(), &mr_meta, id.cf_mod, &cf_meta);
        let name = res
            .candidate
            .as_ref()
            .map(|c| c.project_name.clone())
            .or_else(|| known.first().map(|k| k.project_name.clone()))
            .unwrap_or_else(|| m.display_name());
        seen_names.insert(norm_name(&name));
        seen_names.insert(norm_name(&m.display_name()));
        if let Some(meta) = &m.meta {
            seen_names.insert(norm_name(&meta.mod_id));
        }
        if let Some(c) = &res.candidate {
            match c.source {
                Source::Modrinth => {
                    seen_mr.insert(c.project_id.clone());
                }
                Source::Curseforge => {
                    if let Ok(n) = c.project_id.parse() {
                        seen_cf.insert(n);
                    }
                }
            }
        }
        for (p, v) in &res.mr_deps {
            pending_mr.push((p.clone(), v.clone(), name.clone()));
        }
        for d in &res.cf_deps {
            pending_cf.push((*d, name.clone()));
        }
        rows.push(build_row(
            m.sha1.clone(),
            name,
            Some(m.clone()),
            vec![],
            id.match_kind.unwrap_or(MatchKind::None),
            known,
            res,
            target,
            cf_enabled,
        ));
    }

    // ---- 5. Abhängigkeiten ---------------------------------------------------------------
    if target.include_dependencies {
        let mut round = 0;
        while (!pending_mr.is_empty() || !pending_cf.is_empty()) && round < MAX_DEP_ROUNDS {
            round += 1;
            check_cancel(cancel)?;
            progress(Progress {
                phase: "dependencies".into(),
                done: round,
                total: MAX_DEP_ROUNDS,
                label: "Abhängigkeiten werden aufgelöst …".into(),
            });

            // Modrinth: fehlende Projekt-IDs über Versions-ID nachschlagen
            let mut mr_new: HashMap<String, Vec<String>> = HashMap::new();
            for (p, v, by) in std::mem::take(&mut pending_mr) {
                let pid = match (p, v) {
                    (Some(p), _) => Some(p),
                    (None, Some(v)) => clients
                        .modrinth
                        .version(&v)
                        .await
                        .ok()
                        .flatten()
                        .map(|ver| ver.project_id),
                    _ => None,
                };
                if let Some(pid) = pid {
                    if seen_mr.contains(&pid) {
                        // Bereits vorhanden – nur „benötigt von“ ergänzen
                        add_required_by_mr(&mut rows, &pid, &by);
                        continue;
                    }
                    let e = mr_new.entry(pid).or_default();
                    if !e.contains(&by) {
                        e.push(by);
                    }
                }
            }
            let mut cf_new: HashMap<u64, Vec<String>> = HashMap::new();
            for (mid, by) in std::mem::take(&mut pending_cf) {
                if seen_cf.contains(&mid) {
                    add_required_by_cf(&mut rows, mid, &by);
                    continue;
                }
                let e = cf_new.entry(mid).or_default();
                if !e.contains(&by) {
                    e.push(by);
                }
            }
            if mr_new.is_empty() && cf_new.is_empty() {
                break;
            }

            let ids: Vec<String> = mr_new.keys().cloned().collect();
            if let Ok(m) = clients.modrinth.projects(&ids).await {
                mr_meta.extend(m);
            }
            let ids: Vec<u64> = cf_new.keys().copied().collect();
            if let (false, Some(cf)) = (ids.is_empty(), &clients.curseforge) {
                if let Ok(m) = cf.mods(&ids).await {
                    cf_meta.extend(m);
                }
            }

            let mut jobs: Vec<(Option<String>, Option<u64>, Vec<String>)> = vec![];
            for (pid, by) in mr_new {
                seen_mr.insert(pid.clone());
                jobs.push((Some(pid), None, by));
            }
            for (mid, by) in cf_new {
                seen_cf.insert(mid);
                jobs.push((None, Some(mid), by));
            }
            // Gleiche Mod auf beiden Plattformen (z. B. Fabric API) nicht doppelt aufnehmen
            jobs.retain(|(mr, cf, _)| {
                let name = mr
                    .as_ref()
                    .and_then(|p| mr_meta.get(p).map(|x| x.title.clone()))
                    .or_else(|| cf.and_then(|c| cf_meta.get(&c).map(|x| x.name.clone())));
                match name {
                    Some(n) => seen_names.insert(norm_name(&n)),
                    None => true,
                }
            });

            let mr_meta_ref = &mr_meta;
            let cf_meta_ref = &cf_meta;
            let futs: Vec<_> = jobs
                .into_iter()
                .map(|(mr, cf, by)| async move {
                    let res = resolve_project(
                        clients,
                        target,
                        mr.as_deref(),
                        mr.as_ref().and_then(|p| mr_meta_ref.get(p)),
                        cf,
                        cf.and_then(|c| cf_meta_ref.get(&c)),
                    )
                    .await;
                    (mr, cf, by, res)
                })
                .collect();
            let results: Vec<_> = stream::iter(futs).buffer_unordered(PARALLEL).collect().await;

            for (mr, cf, by, res) in results {
                let known = known_on(mr.as_deref(), &mr_meta, cf, &cf_meta);
                let name = res
                    .candidate
                    .as_ref()
                    .map(|c| c.project_name.clone())
                    .or_else(|| known.first().map(|k| k.project_name.clone()))
                    .unwrap_or_else(|| "Unbekannte Abhängigkeit".into());
                for (p, v) in &res.mr_deps {
                    pending_mr.push((p.clone(), v.clone(), name.clone()));
                }
                for d in &res.cf_deps {
                    pending_cf.push((*d, name.clone()));
                }
                let key = match (&mr, cf) {
                    (Some(p), _) => format!("dep:mr:{p}"),
                    (None, Some(c)) => format!("dep:cf:{c}"),
                    _ => continue,
                };
                rows.push(build_row(
                    key,
                    name,
                    None,
                    by,
                    MatchKind::Dependency,
                    known,
                    &res,
                    target,
                    cf_enabled,
                ));
            }
        }
    }

    Ok(CheckReport { rows, warnings })
}

fn add_required_by_mr(rows: &mut [ResultRow], pid: &str, by: &str) {
    for r in rows.iter_mut() {
        let hit = r
            .known_on
            .iter()
            .any(|k| k.source == Source::Modrinth && k.project_id == pid);
        if hit && r.match_kind == MatchKind::Dependency && !r.required_by.iter().any(|b| b == by) {
            r.required_by.push(by.to_string());
        }
    }
}

fn add_required_by_cf(rows: &mut [ResultRow], mid: u64, by: &str) {
    let mid = mid.to_string();
    for r in rows.iter_mut() {
        let hit = r
            .known_on
            .iter()
            .any(|k| k.source == Source::Curseforge && k.project_id == mid);
        if hit && r.match_kind == MatchKind::Dependency && !r.required_by.iter().any(|b| b == by) {
            r.required_by.push(by.to_string());
        }
    }
}
