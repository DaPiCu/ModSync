//! Passt ein Fabric-Gradle-Projekt an eine andere Minecraft-Version an:
//! gradle.properties, build.gradle(.kts), libs.versions.toml, Gradle-Wrapper und fabric.mod.json.
//! Am Java-Code selbst wird nichts verändert.

use crate::template::Template;
use regex::{Captures, Regex};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mappings {
    /// Fabric-eigene Yarn-Namen
    Yarn,
    /// Offizielle Mojang-Namen
    Mojang,
    /// Geschichtete Mappings (z. B. Parchment)
    Layered,
    /// Keine Mappings (nicht obfuskiertes Minecraft, 26.1+)
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub build_files: Vec<String>,
    pub property_files: Vec<String>,
    pub catalog: Option<String>,
    pub fmj_files: Vec<String>,
    pub wrapper: Option<String>,
    pub has_gradlew: bool,
    pub has_gradlew_bat: bool,
    pub mappings: Mappings,
    pub has_loom: bool,
    pub architectury: bool,
    pub source_minecraft: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub file: String,
    pub what: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PatchReport {
    pub changes: Vec<Change>,
    pub warnings: Vec<String>,
    /// Gründe, warum ein Build sinnlos ist – dann wird nichts gebaut.
    pub blockers: Vec<String>,
}

const SKIP_DIRS: &[&str] = &[".git", ".gradle", "build", "out", "run", "node_modules", ".idea", "bin"];

fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, depth + 1, out);
            }
        } else {
            out.push(p);
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn read(root: &Path, rel_path: &str) -> String {
    std::fs::read_to_string(root.join(rel_path)).unwrap_or_default()
}

pub fn analyze(root: &Path) -> Result<ProjectInfo, String> {
    let mut files = Vec::new();
    walk(root, 0, &mut files);
    let rels: Vec<String> = files.iter().map(|p| rel(root, p)).collect();
    let by_name = |n: &str| -> Vec<String> {
        let mut v: Vec<String> = rels
            .iter()
            .filter(|r| r.rsplit('/').next() == Some(n))
            .cloned()
            .collect();
        v.sort_by_key(|r| (r.matches('/').count(), r.clone()));
        v
    };
    let mut build_files = by_name("build.gradle");
    build_files.extend(by_name("build.gradle.kts"));
    if build_files.is_empty() {
        return Err("Kein Gradle-Projekt gefunden (keine build.gradle).".into());
    }
    let property_files = by_name("gradle.properties");
    let catalog = rels
        .iter()
        .find(|r| r.as_str() == "gradle/libs.versions.toml")
        .cloned();
    let fmj_files: Vec<String> = by_name("fabric.mod.json")
        .into_iter()
        .filter(|r| r.contains("/resources/") || r.contains("/templates/"))
        .collect();
    let wrapper = rels
        .iter()
        .find(|r| r.as_str() == "gradle/wrapper/gradle-wrapper.properties")
        .cloned();

    let all_build: String = build_files.iter().map(|f| read(root, f)).collect::<Vec<_>>().join("\n");
    let all_settings = read(root, "settings.gradle") + &read(root, "settings.gradle.kts");
    let catalog_text = catalog.as_ref().map(|c| read(root, c)).unwrap_or_default();
    let props_text: String = property_files.iter().map(|f| read(root, f)).collect::<Vec<_>>().join("\n");

    let mappings_line = Regex::new(r"(?m)^\s*mappings\s*[\(\s]").unwrap();
    let mappings = if all_build.contains("loom.layered") {
        Mappings::Layered
    } else if all_build.contains("officialMojangMappings") {
        Mappings::Mojang
    } else if all_build.contains("net.fabricmc:yarn")
        || all_build.contains("yarn_mappings")
        || mappings_line.is_match(&all_build)
    {
        Mappings::Yarn
    } else {
        Mappings::None
    };

    let combined = format!("{all_build}\n{all_settings}\n{catalog_text}");
    let has_loom = combined.contains("fabric-loom");
    let architectury = combined.contains("architectury");

    let source_minecraft = crate::template::parse_props(&props_text)
        .into_iter()
        .find(|(k, _)| matches!(k.as_str(), "minecraft_version" | "mc_version"))
        .map(|(_, v)| v);

    Ok(ProjectInfo {
        build_files,
        property_files,
        catalog,
        fmj_files,
        wrapper,
        has_gradlew: root.join("gradlew").is_file(),
        has_gradlew_bat: root.join("gradlew.bat").is_file(),
        mappings,
        has_loom,
        architectury,
        source_minecraft,
    })
}

/// Prüft vorab, ob ein automatischer Versuch überhaupt Sinn ergibt.
pub fn blockers(info: &ProjectInfo, tpl: &Template, yarn: Option<&str>) -> Vec<String> {
    let mut b = Vec::new();
    let wrapper_ok = if cfg!(windows) { info.has_gradlew_bat } else { info.has_gradlew };
    if !wrapper_ok {
        b.push("Das Projekt hat keinen Gradle-Wrapper (gradlew) – automatischer Build nicht möglich.".into());
    }
    if !info.has_loom {
        b.push("Kein Fabric-Loom-Projekt gefunden (eventuell eine Forge-/NeoForge-Mod).".into());
    }
    if info.architectury {
        b.push("Multiloader-Projekt (Architectury) – wird nicht automatisch unterstützt.".into());
    }
    match (tpl.unobfuscated(), info.mappings) {
        (true, Mappings::Yarn) => b.push(format!(
            "Die Mod nutzt Yarn-Namen. Ab Minecraft 26.1 gibt es nur noch die offiziellen Mojang-Namen – \
             dafür müsste der gesamte Code umbenannt werden. Automatisch für {} nicht möglich.",
            tpl.minecraft
        )),
        (true, Mappings::Layered) => b.push(
            "Die Mod nutzt geschichtete Mappings (z. B. Parchment). Für 26.x müssen die manuell entfernt werden.".into(),
        ),
        (false, Mappings::None) => b.push(
            "Die Mod ist bereits für 26.x (ohne Mappings) – eine Rückportierung auf ältere Versionen wird nicht unterstützt."
                .into(),
        ),
        (false, Mappings::Yarn) if yarn.is_none() => b.push(format!(
            "Die Mod nutzt Yarn-Namen, aber für {} gibt es keine Yarn-Mappings.",
            tpl.minecraft
        )),
        _ => {}
    }
    b
}

struct Ctx<'a> {
    changes: &'a mut Vec<Change>,
    file: String,
}

impl Ctx<'_> {
    fn record(&mut self, what: impl Into<String>, before: impl Into<String>, after: impl Into<String>) {
        let (before, after) = (before.into(), after.into());
        if before != after {
            self.changes.push(Change {
                file: self.file.clone(),
                what: what.into(),
                before,
                after,
            });
        }
    }

    /// Ersetzt alle Treffer; `f` liefert den neuen Text für den gesamten Treffer.
    fn replace(&mut self, text: &str, re: &Regex, what: &str, f: impl Fn(&Captures) -> String) -> String {
        let mut changes = Vec::new();
        let out = re
            .replace_all(text, |c: &Captures| {
                let new = f(c);
                if new != c[0] {
                    changes.push((c[0].trim().to_string(), new.trim().to_string()));
                }
                new
            })
            .to_string();
        for (b, a) in changes {
            self.record(what, b, a);
        }
        out
    }
}

fn norm_key(k: &str) -> String {
    k.trim().to_lowercase().replace(['-', '.'], "_")
}

enum Slot {
    Mc,
    Loader,
    Loom,
    Fapi,
    Yarn,
    Java,
}

fn slot_for(key: &str) -> Option<Slot> {
    match norm_key(key).as_str() {
        "minecraft_version" | "mc_version" | "minecraft" | "mc" => Some(Slot::Mc),
        "loader_version" | "fabric_loader_version" | "fabric_loader" | "loader" => Some(Slot::Loader),
        "loom_version" | "fabric_loom_version" | "loom" | "fabric_loom" => Some(Slot::Loom),
        "fabric_version" | "fabric_api_version" | "fabric_api" | "fapi_version" | "fabricapi_version"
        | "fabric" | "fapi" => Some(Slot::Fapi),
        "yarn_mappings" | "yarn_version" | "yarn" => Some(Slot::Yarn),
        "java_version" => Some(Slot::Java),
        _ => None,
    }
}

const IGNORED_VERSION_KEYS: &[&str] = &["mod_version", "version", "maven_version", "mod_api_version"];

struct Values<'a> {
    tpl: &'a Template,
    yarn: Option<&'a str>,
    mappings: Mappings,
}

impl Values<'_> {
    fn get(&self, s: &Slot) -> Option<String> {
        match s {
            Slot::Mc => Some(self.tpl.minecraft.clone()),
            Slot::Loader => Some(self.tpl.loader.clone()),
            Slot::Loom => Some(self.tpl.loom.clone()),
            Slot::Fapi => Some(self.tpl.fabric_api.clone()),
            Slot::Yarn => match self.mappings {
                Mappings::Yarn => self.yarn.map(String::from),
                _ => None,
            },
            Slot::Java => Some(self.tpl.java.to_string()),
        }
    }
}

fn patch_properties(text: &str, ctx: &mut Ctx, vals: &Values, other: &mut Vec<String>) -> String {
    let re = Regex::new(r"(?m)^([ \t]*)([\w.\-]+)([ \t]*[=:][ \t]*)(.*?)([ \t]*)$").unwrap();
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => match b.strip_suffix('\r') {
                Some(b2) => (b2, "\r\n"),
                None => (b, "\n"),
            },
            None => (line, ""),
        };
        if body.trim_start().starts_with('#') {
            out.push_str(line);
            continue;
        }
        if let Some(c) = re.captures(body) {
            let key = &c[2];
            if let Some(slot) = slot_for(key) {
                if let Some(v) = vals.get(&slot) {
                    let old = c[4].to_string();
                    if old != v && !old.contains("${") {
                        ctx.record(key, old, v.clone());
                        out.push_str(&format!("{}{}{}{}{}", &c[1], key, &c[3], v, nl));
                        continue;
                    }
                }
            } else {
                let k = norm_key(key);
                if k.ends_with("_version")
                    && !IGNORED_VERSION_KEYS.contains(&k.as_str())
                    && !k.starts_with("org_gradle")
                    && !other.contains(&key.to_string())
                {
                    other.push(key.to_string());
                }
            }
        }
        out.push_str(line);
    }
    out
}

fn patch_catalog(text: &str, ctx: &mut Ctx, vals: &Values) -> String {
    let mut section = String::new();
    let line_re = Regex::new(r#"^(\s*)([\w.\-]+)(\s*=\s*)"([^"]*)"(.*)$"#).unwrap();
    let id_re = Regex::new(r#"id\s*=\s*"((?:net\.fabricmc\.)?fabric-loom(?:-remap)?)""#).unwrap();
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.trim_matches(['[', ']']).to_string();
            out.push_str(line);
            continue;
        }
        let nl = if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" };
        let body = line.trim_end_matches(['\r', '\n']);
        if section == "versions" {
            if let Some(c) = line_re.captures(body) {
                if let Some(v) = slot_for(&c[2]).and_then(|s| vals.get(&s)) {
                    if c[4] != v {
                        ctx.record(format!("[versions] {}", &c[2]), &c[4], v.clone());
                        out.push_str(&format!("{}{}{}\"{}\"{}{}", &c[1], &c[2], &c[3], v, &c[5], nl));
                        continue;
                    }
                }
            }
        }
        if section == "plugins" {
            if let Some(c) = id_re.captures(body) {
                if c[1] != vals.tpl.plugin_id {
                    ctx.record("Loom-Plugin", &c[1], vals.tpl.plugin_id.clone());
                    let new = id_re.replace(body, format!("id = \"{}\"", vals.tpl.plugin_id).as_str());
                    out.push_str(&format!("{new}{nl}"));
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    out
}

fn patch_build(text: &str, ctx: &mut Ctx, vals: &Values, warnings: &mut Vec<String>) -> String {
    let tpl = vals.tpl;
    let mut s = text.to_string();

    // Loom-Plugin-ID und (literale) Version
    let plugin = Regex::new(
        r#"(id\s*\(?\s*)(['"])((?:net\.fabricmc\.)?fabric-loom(?:-remap)?)(['"])(\s*\)?)(\s*version\s*\(?\s*)?(['"]([^'"]*)['"])?"#,
    )
    .unwrap();
    s = ctx.replace(&s, &plugin, "Loom-Plugin", |c| {
        let q = &c[2];
        let mut out = format!("{}{q}{}{q}{}", &c[1], tpl.plugin_id, &c[5]);
        if let Some(vkw) = c.get(6) {
            out.push_str(vkw.as_str());
            if let Some(ver) = c.get(8) {
                let ver = ver.as_str();
                if ver.contains('$') {
                    out.push_str(&c[7]);
                } else {
                    out.push_str(&format!("{q}{}{q}", tpl.loom));
                }
            }
        }
        out
    });
    let apply_plugin = Regex::new(r#"(apply\s+plugin\s*:\s*['"])(fabric-loom)(['"])"#).unwrap();
    s = ctx.replace(&s, &apply_plugin, "Loom-Plugin", |c| format!("{}{}{}", &c[1], tpl.plugin_id, &c[3]));

    // Literale Versionen in Abhängigkeiten
    let mc = Regex::new(r#"(com\.mojang:minecraft:)([0-9][^"'$\s)]*)"#).unwrap();
    s = ctx.replace(&s, &mc, "Minecraft-Version", |c| format!("{}{}", &c[1], tpl.minecraft));
    let loader = Regex::new(r#"(net\.fabricmc:fabric-loader:)([0-9][^"'$\s)]*)"#).unwrap();
    s = ctx.replace(&s, &loader, "Fabric Loader", |c| format!("{}{}", &c[1], tpl.loader));
    let fapi = Regex::new(r#"(net\.fabricmc\.fabric-api:fabric-api:)([0-9][^"'$\s)]*)"#).unwrap();
    s = ctx.replace(&s, &fapi, "Fabric API", |c| format!("{}{}", &c[1], tpl.fabric_api));
    if let (Mappings::Yarn, Some(y)) = (vals.mappings, vals.yarn) {
        let yarn = Regex::new(r#"(net\.fabricmc:yarn:)([0-9][^"'$\s):]*)"#).unwrap();
        s = ctx.replace(&s, &yarn, "Yarn-Mappings", |c| format!("{}{}", &c[1], y));
    }

    if tpl.unobfuscated() {
        // Mojang-Mappings-Zeile entfällt
        let mappings = Regex::new(
            r"(?m)^[ \t]*mappings[ \t]*\(?[ \t]*loom\.officialMojangMappings\(\)[ \t]*\)?[ \t]*(?://[^\n]*)?\r?\n?",
        )
        .unwrap();
        s = ctx.replace(&s, &mappings, "Mappings entfernt (26.x hat keine Obfuskierung)", |_| String::new());
        // mod*-Konfigurationen gibt es ohne Remapping nicht mehr
        let modcfg =
            Regex::new(r"\bmod(Implementation|Api|CompileOnly|CompileOnlyApi|RuntimeOnly|LocalRuntime)\b").unwrap();
        s = ctx.replace(&s, &modcfg, "Abhängigkeits-Konfiguration", |c| {
            let n = &c[1];
            let mut chars = n.chars();
            let first = chars.next().unwrap().to_lowercase().to_string();
            format!("{first}{}", chars.as_str())
        });
        if Regex::new(r"\bremap(Jar|SourcesJar)\b").unwrap().is_match(&s) {
            warnings.push(format!(
                "{}: verweist auf remapJar/remapSourcesJar – diese Tasks gibt es in 26.x nicht mehr und müssen ggf. manuell angepasst werden.",
                ctx.file
            ));
        }
    }

    // Java-Version
    let j = tpl.java.to_string();
    let jv = Regex::new(r"JavaVersion\.VERSION_(\d+)").unwrap();
    s = ctx.replace(&s, &jv, "Java-Version", |c| {
        if c[1] == j { c[0].to_string() } else { format!("JavaVersion.VERSION_{j}") }
    });
    let jr = Regex::new(r"(\brelease\s*=\s*|\brelease\.set\(\s*|JavaLanguageVersion\.of\(\s*)(\d+)").unwrap();
    s = ctx.replace(&s, &jr, "Java-Version", |c| format!("{}{}", &c[1], j));
    let kt = Regex::new(r"JvmTarget\.JVM_(\d+)").unwrap();
    s = ctx.replace(&s, &kt, "Kotlin-JVM-Ziel", |_| format!("JvmTarget.JVM_{j}"));
    s
}

fn patch_wrapper(text: &str, ctx: &mut Ctx, tpl: &Template) -> String {
    let Some(dist) = &tpl.gradle_distribution else { return text.to_string() };
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        let nl = if line.ends_with("\r\n") { "\r\n" } else if line.ends_with('\n') { "\n" } else { "" };
        if t.starts_with("distributionUrl=") {
            if t != dist {
                ctx.record("Gradle-Version", t.trim_start_matches("distributionUrl="), dist.trim_start_matches("distributionUrl="));
            }
            out.push_str(dist);
            out.push_str(nl);
        } else if t.starts_with("distributionSha256Sum=") {
            ctx.record("Prüfsumme der alten Gradle-Version entfernt", t, "");
        } else {
            out.push_str(line);
        }
    }
    out
}

fn sanitize_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut in_str, mut esc) = (false, false);
    for ch in text.trim_start_matches('\u{feff}').chars() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            } else if (ch as u32) < 0x20 {
                out.push(' ');
                continue;
            }
        } else if ch == '"' {
            in_str = true;
        }
        out.push(ch);
    }
    out
}

fn patch_fmj(text: &str, ctx: &mut Ctx, tpl: &Template, warnings: &mut Vec<String>) -> Option<String> {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&sanitize_json(text)) else {
        warnings.push(format!("{}: konnte nicht gelesen werden und wurde nicht angepasst.", ctx.file));
        return None;
    };
    let deps = v.get_mut("depends").and_then(|d| d.as_object_mut())?;
    let mut changed = false;
    let mut set = |key: &str, new: String, ctx: &mut Ctx| {
        if let Some(serde_json::Value::String(old)) = deps.get(key) {
            if !old.contains("${") && *old != new {
                ctx.record(format!("depends.{key}"), old.clone(), new.clone());
                deps.insert(key.to_string(), serde_json::Value::String(new));
                changed = true;
            }
        } else if let Some(serde_json::Value::Array(_)) = deps.get(key) {
            ctx.record(format!("depends.{key}"), "[…]", new.clone());
            deps.insert(key.to_string(), serde_json::Value::String(new));
            changed = true;
        }
    };
    set("minecraft", tpl.fmj_minecraft.clone(), ctx);
    set("fabricloader", format!(">={}", tpl.loader), ctx);
    if let Some(j) = &tpl.fmj_java {
        set("java", j.clone(), ctx);
    }
    if !changed {
        return None;
    }
    // Einrückung des Originals beibehalten (Tabs oder Leerzeichen)
    let indent: Vec<u8> = text
        .lines()
        .nth(1)
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').collect::<String>())
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| "  ".into())
        .into_bytes();
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(&indent);
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    serde::Serialize::serialize(&v, &mut ser).ok()?;
    let mut out = String::from_utf8(buf).ok()?;
    out.push('\n');
    Some(out)
}

/// Wendet alle Anpassungen auf die Arbeitskopie an.
pub fn apply(root: &Path, info: &ProjectInfo, tpl: &Template, yarn: Option<&str>) -> Result<PatchReport, String> {
    let mut report = PatchReport {
        blockers: blockers(info, tpl, yarn),
        ..Default::default()
    };
    if !report.blockers.is_empty() {
        return Ok(report);
    }
    let vals = Values { tpl, yarn, mappings: info.mappings };
    let mut other_versions = Vec::new();
    let write = |f: &str, s: &str| {
        std::fs::write(root.join(f), s).map_err(|e| format!("{f} kann nicht geschrieben werden: {e}"))
    };

    for f in &info.property_files {
        let text = read(root, f);
        let mut ctx = Ctx { changes: &mut report.changes, file: f.clone() };
        let new = patch_properties(&text, &mut ctx, &vals, &mut other_versions);
        if new != text {
            write(f, &new)?;
        }
    }
    if let Some(f) = &info.catalog {
        let text = read(root, f);
        let mut ctx = Ctx { changes: &mut report.changes, file: f.clone() };
        let new = patch_catalog(&text, &mut ctx, &vals);
        if new != text {
            write(f, &new)?;
        }
    }
    for f in &info.build_files {
        let text = read(root, f);
        let mut ctx = Ctx { changes: &mut report.changes, file: f.clone() };
        let new = patch_build(&text, &mut ctx, &vals, &mut report.warnings);
        if new != text {
            write(f, &new)?;
        }
    }
    if let Some(f) = &info.wrapper {
        let text = read(root, f);
        let mut ctx = Ctx { changes: &mut report.changes, file: f.clone() };
        let new = patch_wrapper(&text, &mut ctx, tpl);
        if new != text {
            write(f, &new)?;
        }
    } else {
        report.warnings.push("Keine gradle-wrapper.properties gefunden – Gradle-Version bleibt unverändert.".into());
    }
    for f in &info.fmj_files {
        let text = read(root, f);
        let mut ctx = Ctx { changes: &mut report.changes, file: f.clone() };
        if let Some(new) = patch_fmj(&text, &mut ctx, tpl, &mut report.warnings) {
            write(f, &new)?;
        }
    }
    if info.fmj_files.is_empty() {
        report.warnings.push("Keine fabric.mod.json gefunden.".into());
    }
    if !other_versions.is_empty() {
        report.warnings.push(format!(
            "Abhängigkeiten zu anderen Mods wurden nicht angepasst: {}. Falls der Build daran scheitert, müssen diese Versionen von Hand auf {} gesetzt werden.",
            other_versions.join(", "),
            tpl.minecraft
        ));
    }
    if report.changes.is_empty() {
        report.warnings.push("Es wurde nichts geändert – das Projekt passt evtl. schon zur Zielversion.".into());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::parse_template;

    fn tpl(dir: &str) -> Template {
        let p = |f: &str| {
            std::fs::read_to_string(format!("{}/tests/fixtures/{dir}/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap()
        };
        parse_template(
            &p("gradle.properties"),
            &p("build.gradle"),
            Some(&p("gradle-wrapper.properties")),
            Some(&p("fabric.mod.json")),
        )
        .unwrap()
    }

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, c) in files {
            let path = d.path().join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        }
        std::fs::write(d.path().join("gradlew"), "#!/bin/sh").unwrap();
        std::fs::write(d.path().join("gradlew.bat"), "@echo off").unwrap();
        d
    }

    const MOJ_BUILD: &str = r#"plugins {
	id 'fabric-loom' version '1.7-SNAPSHOT'
	id 'maven-publish'
}
dependencies {
	minecraft "com.mojang:minecraft:${project.minecraft_version}"
	mappings loom.officialMojangMappings()
	modImplementation "net.fabricmc:fabric-loader:${project.loader_version}"
	modImplementation "net.fabricmc.fabric-api:fabric-api:${project.fabric_version}"
	modCompileOnly "com.terraformersmc:modmenu:${project.modmenu_version}"
	include modImplementation("me.lucko:fabric-permissions-api:0.3.1")
}
tasks.withType(JavaCompile).configureEach {
	it.options.release = 21
}
java {
	sourceCompatibility = JavaVersion.VERSION_21
	targetCompatibility = JavaVersion.VERSION_21
}
"#;

    const PROPS: &str = "org.gradle.jvmargs=-Xmx1G\n# Fabric\nminecraft_version=1.21.1\nloader_version=0.16.5\n\nmod_version=1.2.0\nfabric_version=0.105.0+1.21.1\nmodmenu_version=11.0.2\n";

    const FMJ: &str = r#"{
  "schemaVersion": 1,
  "id": "testmod",
  "version": "${version}",
  "description": "line
break",
  "depends": { "fabricloader": ">=0.16.5", "minecraft": "~1.21.1", "java": ">=21", "fabric-api": "*" }
}"#;

    const WRAPPER: &str = "distributionBase=GRADLE_USER_HOME\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-8.10-bin.zip\ndistributionSha256Sum=abc\n";

    #[test]
    fn mojang_mod_to_26_2() {
        let d = project(&[
            ("build.gradle", MOJ_BUILD),
            ("gradle.properties", PROPS),
            ("src/main/resources/fabric.mod.json", FMJ),
            ("gradle/wrapper/gradle-wrapper.properties", WRAPPER),
        ]);
        let info = analyze(d.path()).unwrap();
        assert_eq!(info.mappings, Mappings::Mojang);
        assert_eq!(info.source_minecraft.as_deref(), Some("1.21.1"));
        let t = tpl("template-26.2");
        let r = apply(d.path(), &info, &t, None).unwrap();
        assert!(r.blockers.is_empty(), "{:?}", r.blockers);

        let build = std::fs::read_to_string(d.path().join("build.gradle")).unwrap();
        assert!(build.contains(&format!("id 'net.fabricmc.fabric-loom' version '{}'", t.loom)), "{build}");
        assert!(!build.contains("mappings"));
        assert!(!build.contains("modImplementation"));
        assert!(build.contains("compileOnly \"com.terraformersmc"));
        assert!(build.contains("include implementation(\"me.lucko"));
        assert!(build.contains("release = 25"));
        assert!(build.contains("JavaVersion.VERSION_25"));

        let props = std::fs::read_to_string(d.path().join("gradle.properties")).unwrap();
        assert!(props.contains("minecraft_version=26.2\n"));
        assert!(props.contains(&format!("fabric_version={}\n", t.fabric_api)));
        assert!(props.contains("mod_version=1.2.0"));
        assert!(props.contains("# Fabric"));

        let fmj = std::fs::read_to_string(d.path().join("src/main/resources/fabric.mod.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fmj).unwrap();
        assert_eq!(v["depends"]["minecraft"], "~26.2");
        assert_eq!(v["depends"]["java"], ">=25");
        assert_eq!(v["version"], "${version}");

        let w = std::fs::read_to_string(d.path().join("gradle/wrapper/gradle-wrapper.properties")).unwrap();
        assert!(w.contains(t.gradle_version.as_ref().unwrap()));
        assert!(!w.contains("Sha256"));

        assert!(r.warnings.iter().any(|w| w.contains("modmenu_version")), "{:?}", r.warnings);
        assert!(r.changes.len() >= 8, "{:#?}", r.changes);
    }

    #[test]
    fn yarn_mod_to_26_is_blocked() {
        let build = MOJ_BUILD.replace(
            "mappings loom.officialMojangMappings()",
            "mappings \"net.fabricmc:yarn:${project.yarn_mappings}:v2\"",
        );
        let d = project(&[("build.gradle", &build), ("gradle.properties", PROPS)]);
        let info = analyze(d.path()).unwrap();
        assert_eq!(info.mappings, Mappings::Yarn);
        let r = apply(d.path(), &info, &tpl("template-26.2"), None).unwrap();
        assert_eq!(r.blockers.len(), 1);
        assert!(r.blockers[0].contains("Yarn"));
        // Nichts verändert
        assert_eq!(std::fs::read_to_string(d.path().join("build.gradle")).unwrap(), build);
    }

    #[test]
    fn yarn_mod_to_older_target_updates_yarn() {
        let build = MOJ_BUILD.replace(
            "mappings loom.officialMojangMappings()",
            "mappings \"net.fabricmc:yarn:${project.yarn_mappings}:v2\"",
        );
        let props = format!("{PROPS}yarn_mappings=1.21.1+build.3\n");
        let d = project(&[("build.gradle", &build), ("gradle.properties", &props)]);
        let info = analyze(d.path()).unwrap();
        let t = tpl("template-1.21.4");
        assert!(!blockers(&info, &t, None).is_empty());
        let r = apply(d.path(), &info, &t, Some("1.21.4+build.8")).unwrap();
        assert!(r.blockers.is_empty());
        let p = std::fs::read_to_string(d.path().join("gradle.properties")).unwrap();
        assert!(p.contains("yarn_mappings=1.21.4+build.8"));
        let b = std::fs::read_to_string(d.path().join("build.gradle")).unwrap();
        assert!(b.contains("modImplementation"), "remap-Ziel behält mod*-Konfigurationen");
        assert!(b.contains("id 'net.fabricmc.fabric-loom-remap'"));
    }

    #[test]
    fn kotlin_dsl_and_catalog() {
        let kts = r#"plugins {
    alias(libs.plugins.loom)
}
dependencies {
    minecraft("com.mojang:minecraft:1.21.1")
    mappings(loom.officialMojangMappings())
    modImplementation(libs.fabric.loader)
}
java { toolchain.languageVersion.set(JavaLanguageVersion.of(21)) }
"#;
        let catalog = "[versions]\nminecraft = \"1.21.1\"\nfabric-loader = \"0.16.5\"\nfabric-api = \"0.105.0+1.21.1\"\nloom = \"1.7-SNAPSHOT\"\n\n[plugins]\nloom = { id = \"fabric-loom\", version.ref = \"loom\" }\n";
        let d = project(&[("build.gradle.kts", kts), ("gradle/libs.versions.toml", catalog)]);
        let info = analyze(d.path()).unwrap();
        assert!(info.has_loom);
        let t = tpl("template-26.2");
        let r = apply(d.path(), &info, &t, None).unwrap();
        assert!(r.blockers.is_empty(), "{:?}", r.blockers);
        let c = std::fs::read_to_string(d.path().join("gradle/libs.versions.toml")).unwrap();
        assert!(c.contains("minecraft = \"26.2\""));
        assert!(c.contains(&format!("loom = \"{}\"", t.loom)));
        assert!(c.contains("id = \"net.fabricmc.fabric-loom\""));
        let b = std::fs::read_to_string(d.path().join("build.gradle.kts")).unwrap();
        assert!(b.contains("minecraft(\"com.mojang:minecraft:26.2\")"));
        assert!(!b.contains("mappings"));
        assert!(b.contains("implementation(libs.fabric.loader)"));
        assert!(b.contains("JavaLanguageVersion.of(25)"));
    }

    #[test]
    fn architectury_and_missing_wrapper_blocked() {
        let d = project(&[("build.gradle", "plugins { id 'dev.architectury.loom' version '1.7' }\n// fabric-loom")]);
        std::fs::remove_file(d.path().join("gradlew")).unwrap();
        std::fs::remove_file(d.path().join("gradlew.bat")).unwrap();
        let info = analyze(d.path()).unwrap();
        let b = blockers(&info, &tpl("template-26.2"), None);
        assert!(b.iter().any(|x| x.contains("Architectury")));
        assert!(b.iter().any(|x| x.contains("Wrapper")));
    }
}
