//! Offizielle Versionsvorlage: Das Fabric-Example-Mod-Repository hat für jede Minecraft-Version
//! einen eigenen Branch mit passenden Versionen für Loader, Fabric API, Loom, Gradle und Java.

use crate::http;
use regex::Regex;
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE: &str = "https://raw.githubusercontent.com/FabricMC/fabric-example-mod";
pub const DEFAULT_META: &str = "https://meta.fabricmc.net";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub minecraft: String,
    pub loader: String,
    pub loom: String,
    pub fabric_api: String,
    pub plugin_id: String,
    /// Mappings-Zeile der Vorlage (z. B. `mappings loom.officialMojangMappings()`), `None` bei 26.x+
    pub mappings_line: Option<String>,
    pub java: u32,
    /// Roh-Zeile `distributionUrl=...` aus gradle-wrapper.properties
    pub gradle_distribution: Option<String>,
    pub gradle_version: Option<String>,
    /// Wert von depends.minecraft in der fabric.mod.json (z. B. "~26.2")
    pub fmj_minecraft: String,
    pub fmj_java: Option<String>,
}

impl Template {
    /// Ab 26.1 ist Minecraft nicht mehr obfuskiert – keine Mappings, Loom ohne Remapping.
    pub fn unobfuscated(&self) -> bool {
        self.mappings_line.is_none()
    }
}

pub fn parse_props(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') || l.starts_with('!') || l.is_empty() {
                return None;
            }
            let idx = l.find(['=', ':'])?;
            Some((l[..idx].trim().to_string(), l[idx + 1..].trim().to_string()))
        })
        .collect()
}

fn prop(props: &[(String, String)], keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| props.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.clone()))
}

pub fn parse_template(
    properties: &str,
    build_gradle: &str,
    wrapper: Option<&str>,
    fabric_mod_json: Option<&str>,
) -> Result<Template, String> {
    let props = parse_props(properties);
    let missing = |what: &str| format!("Vorlage unvollständig: {what} fehlt.");
    let minecraft = prop(&props, &["minecraft_version"]).ok_or_else(|| missing("minecraft_version"))?;
    let loader = prop(&props, &["loader_version"]).ok_or_else(|| missing("loader_version"))?;
    let loom = prop(&props, &["loom_version"]).ok_or_else(|| missing("loom_version"))?;
    let fabric_api = prop(&props, &["fabric_api_version", "fabric_version"])
        .ok_or_else(|| missing("fabric_api_version"))?;

    let plugin_re = Regex::new(r#"id\s*\(?\s*['"]((?:net\.fabricmc\.)?fabric-loom(?:-remap)?)['"]"#).unwrap();
    let plugin_id = plugin_re
        .captures(build_gradle)
        .map(|c| c[1].to_string())
        .ok_or_else(|| missing("Loom-Plugin"))?;

    let map_re = Regex::new(r"(?m)^\s*(mappings\s.*?)\s*$").unwrap();
    let mappings_line = map_re.captures(build_gradle).map(|c| c[1].to_string());

    let java_re = Regex::new(r"(?:release\s*=\s*|VERSION_)(\d+)").unwrap();
    let java = java_re
        .captures(build_gradle)
        .and_then(|c| c[1].parse().ok())
        .ok_or_else(|| missing("Java-Version"))?;

    let gradle_distribution = wrapper.and_then(|w| {
        w.lines()
            .find(|l| l.trim_start().starts_with("distributionUrl="))
            .map(|l| l.trim().to_string())
    });
    let gradle_version = gradle_distribution.as_ref().and_then(|d| {
        Regex::new(r"gradle-([0-9][^-/]*)-(?:bin|all)\.zip")
            .unwrap()
            .captures(d)
            .map(|c| c[1].to_string())
    });

    let fmj: Option<serde_json::Value> = fabric_mod_json.and_then(|t| serde_json::from_str(t).ok());
    let dep = |k: &str| {
        fmj.as_ref()
            .and_then(|v| v.get("depends"))
            .and_then(|d| d.get(k))
            .and_then(|x| x.as_str())
            .map(String::from)
    };
    let fmj_minecraft = dep("minecraft").unwrap_or_else(|| format!("~{minecraft}"));
    let fmj_java = dep("java").or_else(|| Some(format!(">={java}")));

    Ok(Template {
        minecraft,
        loader,
        loom,
        fabric_api,
        plugin_id,
        mappings_line,
        java,
        gradle_distribution,
        gradle_version,
        fmj_minecraft,
        fmj_java,
    })
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<Option<String>, String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| http::describe("GitHub", &e))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(http::describe_status("GitHub", resp.status()));
    }
    resp.text()
        .await
        .map(Some)
        .map_err(|e| http::describe("GitHub", &e))
}

pub async fn fetch(base: &str, minecraft: &str) -> Result<Template, String> {
    let client = http::client();
    let base = base.trim_end_matches('/');
    let branch: String = url::form_urlencoded::byte_serialize(minecraft.as_bytes()).collect();
    let url = |f: &str| format!("{base}/{branch}/{f}");
    let props = get_text(&client, &url("gradle.properties"))
        .await?
        .ok_or_else(|| {
            format!(
                "Für Minecraft {minecraft} gibt es (noch) keine offizielle Fabric-Vorlage. \
                 Automatisches Anpassen ist nur für Versionen möglich, die Fabric unterstützt."
            )
        })?;
    let build = get_text(&client, &url("build.gradle"))
        .await?
        .ok_or("Vorlage unvollständig: build.gradle fehlt.")?;
    let wrapper = get_text(&client, &url("gradle/wrapper/gradle-wrapper.properties")).await?;
    let fmj = get_text(&client, &url("src/main/resources/fabric.mod.json")).await?;
    let t = parse_template(&props, &build, wrapper.as_deref(), fmj.as_deref())?;
    if t.minecraft != minecraft {
        return Err(format!(
            "Die Fabric-Vorlage für {minecraft} verweist auf {} – Zielversion prüfen.",
            t.minecraft
        ));
    }
    Ok(t)
}

#[derive(Deserialize)]
struct YarnEntry {
    version: String,
}

/// Neueste Yarn-Mappings für eine Version (gibt es nur bis 1.21.11).
pub async fn latest_yarn(meta_base: &str, minecraft: &str) -> Result<Option<String>, String> {
    let client = http::client();
    let enc: String = url::form_urlencoded::byte_serialize(minecraft.as_bytes()).collect();
    let resp = client
        .get(format!("{}/v2/versions/yarn/{enc}", meta_base.trim_end_matches('/')))
        .send()
        .await
        .map_err(|e| http::describe("Fabric Meta", &e))?;
    if !resp.status().is_success() {
        return Err(http::describe_status("Fabric Meta", resp.status()));
    }
    let list: Vec<YarnEntry> = resp
        .json()
        .await
        .map_err(|e| http::describe("Fabric Meta", &e))?;
    Ok(list.into_iter().next().map(|y| y.version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &str) -> Template {
        let p = |f: &str| {
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/{dir}/{f}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap()
        };
        parse_template(
            &p("gradle.properties"),
            &p("build.gradle"),
            Some(&p("gradle-wrapper.properties")),
            Some(&p("fabric.mod.json")),
        )
        .unwrap()
    }

    #[test]
    fn parses_unobfuscated_template() {
        let t = fixture("template-26.2");
        assert_eq!(t.minecraft, "26.2");
        assert_eq!(t.plugin_id, "net.fabricmc.fabric-loom");
        assert!(t.unobfuscated());
        assert_eq!(t.java, 25);
        assert_eq!(t.fmj_minecraft, "~26.2");
        assert!(t.fabric_api.ends_with("+26.2"));
        assert!(t.gradle_version.is_some());
    }

    #[test]
    fn parses_obfuscated_template() {
        let t = fixture("template-1.21.4");
        assert_eq!(t.minecraft, "1.21.4");
        assert_eq!(t.plugin_id, "net.fabricmc.fabric-loom-remap");
        assert_eq!(t.mappings_line.as_deref(), Some("mappings loom.officialMojangMappings()"));
        assert_eq!(t.java, 21);
    }
}
