//! „Versionssperre aufheben“: Original-JAR installieren und per
//! `config/fabric_loader_dependencies.json` die Minecraft-Abhängigkeit überschreiben.
//! (Offizielle Fabric-Loader-Funktion „Dependency Overrides“.)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideResult {
    pub jar_path: String,
    pub config_path: String,
    pub jar_copied: bool,
}

/// Ob ein Override überhaupt Sinn ergibt. Sprünge über die 26.1-Grenze (Obfuskierung entfernt)
/// können mit alten JARs grundsätzlich nicht funktionieren.
pub fn feasibility(mod_minecraft: Option<&str>, target: &str) -> Result<(), String> {
    let era = |v: &str| -> Option<bool> {
        let digits: String = v.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        let major: u32 = digits.parse().ok()?;
        Some(major >= 26)
    };
    match (mod_minecraft.and_then(era), era(target)) {
        (Some(false), Some(true)) => Err(
            "Die Mod wurde für Minecraft 1.x gebaut. Ab 26.1 ist Minecraft nicht mehr obfuskiert und Fabric \
             löst Namen anders auf – alte JARs laden dort grundsätzlich nicht. Nur „Selbst bauen“ kann helfen."
                .into(),
        ),
        (Some(true), Some(false)) => Err("Rückwärts (26.x → 1.x) funktioniert nicht.".into()),
        _ => Ok(()),
    }
}

/// `mods`-Ordner → `config` daneben.
pub fn config_dir_for(mods_dir: &Path) -> PathBuf {
    mods_dir.parent().map(|p| p.join("config")).unwrap_or_else(|| mods_dir.join("config"))
}

pub fn install(
    jar: &Path,
    mod_id: &str,
    target_version: &str,
    mods_dir: &Path,
) -> Result<OverrideResult, String> {
    if mod_id.is_empty() || !mod_id.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
        return Err("Ungültige Mod-ID.".into());
    }
    let name = jar.file_name().ok_or("Ungültige Datei.")?;
    std::fs::create_dir_all(mods_dir).map_err(|e| format!("Zielordner: {e}"))?;
    let dest = mods_dir.join(name);
    let jar_copied = if dest.exists() {
        let same = std::fs::read(&dest).ok() == std::fs::read(jar).ok();
        if !same {
            return Err("Im Zielordner liegt schon eine andere Datei mit diesem Namen – nicht überschrieben.".into());
        }
        false
    } else {
        std::fs::copy(jar, &dest).map_err(|e| format!("JAR kann nicht kopiert werden: {e}"))?;
        true
    };

    let cfg_dir = config_dir_for(mods_dir);
    std::fs::create_dir_all(&cfg_dir).map_err(|e| format!("config-Ordner: {e}"))?;
    let cfg = cfg_dir.join("fabric_loader_dependencies.json");
    let mut root: serde_json::Value = std::fs::read_to_string(&cfg)
        .ok()
        .map(|t| serde_json::from_str(&t).map_err(|_| "Vorhandene fabric_loader_dependencies.json ist ungültig – bitte prüfen.".to_string()))
        .transpose()?
        .unwrap_or_else(|| serde_json::json!({ "version": 1, "overrides": {} }));
    if root.get("version").is_none() {
        root["version"] = serde_json::json!(1);
    }
    if !root.get("overrides").map(|o| o.is_object()).unwrap_or(false) {
        root["overrides"] = serde_json::json!({});
    }
    let entry = root["overrides"]
        .as_object_mut()
        .unwrap()
        .entry(mod_id.to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !entry.is_object() {
        *entry = serde_json::json!({});
    }
    let plus = entry
        .as_object_mut()
        .unwrap()
        .entry("+depends")
        .or_insert_with(|| serde_json::json!({}));
    if !plus.is_object() {
        *plus = serde_json::json!({});
    }
    plus["minecraft"] = serde_json::json!(format!("~{target_version}"));
    let text = serde_json::to_string_pretty(&root).unwrap() + "\n";
    std::fs::write(&cfg, text).map_err(|e| format!("Konfiguration kann nicht geschrieben werden: {e}"))?;

    Ok(OverrideResult {
        jar_path: dest.to_string_lossy().to_string(),
        config_path: cfg.to_string_lossy().to_string(),
        jar_copied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feasibility_rules() {
        assert!(feasibility(Some("~1.21.1"), "26.2").is_err());
        assert!(feasibility(Some(">=1.21.4"), "1.21.10").is_ok());
        assert!(feasibility(Some("~26.1"), "26.2").is_ok());
        assert!(feasibility(Some("26.1"), "1.21.1").is_err());
        assert!(feasibility(None, "26.2").is_ok());
    }

    #[test]
    fn writes_and_merges_override() {
        let d = tempfile::tempdir().unwrap();
        let inst = d.path().join("instance");
        let mods = inst.join("mods");
        std::fs::create_dir_all(inst.join("config")).unwrap();
        std::fs::write(
            inst.join("config/fabric_loader_dependencies.json"),
            r#"{"version":1,"overrides":{"other":{"-depends":{"minecraft":""}}}}"#,
        )
        .unwrap();
        let jar = d.path().join("cool-1.0.jar");
        std::fs::write(&jar, b"jar").unwrap();
        let r = install(&jar, "coolmod", "1.21.10", &mods).unwrap();
        assert!(r.jar_copied);
        assert!(mods.join("cool-1.0.jar").exists());
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&r.config_path).unwrap()).unwrap();
        assert_eq!(v["overrides"]["coolmod"]["+depends"]["minecraft"], "~1.21.10");
        assert!(v["overrides"]["other"].is_object());
        // zweiter Lauf: gleiche Datei → ok, nichts kopiert
        let r2 = install(&jar, "coolmod", "1.21.10", &mods).unwrap();
        assert!(!r2.jar_copied);
        assert!(install(&jar, "../x", "1", &mods).is_err());
    }
}
