use crate::models::{LocalMod, ModMeta};
use sha1::{Digest, Sha1};
use sha2::Sha512;
use std::io::{Cursor, Read};
use std::path::Path;

/// CurseForge-Fingerprint: MurmurHash2 (Seed 1) über die Datei ohne Whitespace-Bytes (9, 10, 13, 32).
pub fn curseforge_fingerprint(data: &[u8]) -> u32 {
    let filtered: Vec<u8> = data
        .iter()
        .copied()
        .filter(|b| !matches!(b, 9 | 10 | 13 | 32))
        .collect();
    murmur2(&filtered, 1)
}

pub fn murmur2(data: &[u8], seed: u32) -> u32 {
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;
    let mut h: u32 = seed ^ (data.len() as u32);
    let mut chunks = data.chunks_exact(4);
    for c in &mut chunks {
        let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M);
        h ^= k;
    }
    let rem = chunks.remainder();
    match rem.len() {
        3 => {
            h ^= (rem[2] as u32) << 16;
            h ^= (rem[1] as u32) << 8;
            h ^= rem[0] as u32;
            h = h.wrapping_mul(M);
        }
        2 => {
            h ^= (rem[1] as u32) << 8;
            h ^= rem[0] as u32;
            h = h.wrapping_mul(M);
        }
        1 => {
            h ^= rem[0] as u32;
            h = h.wrapping_mul(M);
        }
        _ => {}
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    h
}

pub fn sha1_hex(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

pub fn sha512_hex(data: &[u8]) -> String {
    hex::encode(Sha512::digest(data))
}

/// Liest alle `.jar`-Dateien (nicht rekursiv) aus einem Ordner.
pub fn scan_folder(dir: &Path) -> Result<Vec<LocalMod>, String> {
    if !dir.is_dir() {
        return Err(format!("Ordner nicht gefunden: {}", dir.display()));
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("Ordner kann nicht gelesen werden ({}): {e}", dir.display()))?;
    let mut mods = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_jar = path
            .extension()
            .map(|e| e.eq_ignore_ascii_case("jar"))
            .unwrap_or(false);
        if !is_jar || !path.is_file() {
            continue;
        }
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        mods.push(analyze_jar(&path, &data));
    }
    mods.sort_by_key(|m| m.display_name().to_lowercase());
    Ok(mods)
}

pub fn analyze_jar(path: &Path, data: &[u8]) -> LocalMod {
    LocalMod {
        file_name: path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.to_string_lossy().to_string(),
        size: data.len() as u64,
        sha1: sha1_hex(data),
        sha512: sha512_hex(data),
        cf_fingerprint: curseforge_fingerprint(data),
        meta: read_meta(data),
    }
}

fn read_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    if file.size() > 4 * 1024 * 1024 {
        return None;
    }
    let mut s = String::new();
    file.read_to_string(&mut s).ok()?;
    Some(s.trim_start_matches('\u{feff}').to_string())
}

/// Liest Mod-ID, Name und Version aus fabric.mod.json, quilt.mod.json oder (neoforge.)mods.toml.
pub fn read_meta(data: &[u8]) -> Option<ModMeta> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).ok()?;

    if let Some(text) = read_entry(&mut archive, "fabric.mod.json") {
        if let Some(meta) = parse_fabric(&text) {
            return Some(meta);
        }
    }
    if let Some(text) = read_entry(&mut archive, "quilt.mod.json") {
        if let Some(meta) = parse_quilt(&text) {
            return Some(meta);
        }
    }
    if let Some(text) = read_entry(&mut archive, "META-INF/neoforge.mods.toml") {
        if let Some(meta) = parse_mods_toml(&text, "neoforge") {
            return Some(meta);
        }
    }
    if let Some(text) = read_entry(&mut archive, "META-INF/mods.toml") {
        if let Some(meta) = parse_mods_toml(&text, "forge") {
            return Some(meta);
        }
    }
    None
}

fn json_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn dep_to_string(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) => Some(
            a.iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(" || "),
        ),
        _ => None,
    }
}

pub fn parse_fabric(text: &str) -> Option<ModMeta> {
    let v: serde_json::Value = serde_json::from_str(&sanitize_json(text)).ok()?;
    let mod_id = json_str(&v, "id")?;
    Some(ModMeta {
        loader: "fabric".into(),
        mod_id,
        name: json_str(&v, "name"),
        version: json_str(&v, "version"),
        minecraft: dep_to_string(v.get("depends").and_then(|d| d.get("minecraft"))),
        sources: v
            .get("contact")
            .and_then(|c| c.get("sources"))
            .and_then(|s| s.as_str())
            .map(String::from),
    })
}

pub fn parse_quilt(text: &str) -> Option<ModMeta> {
    let v: serde_json::Value = serde_json::from_str(&sanitize_json(text)).ok()?;
    let ql = v.get("quilt_loader")?;
    let mod_id = json_str(ql, "id")?;
    let name = ql
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from);
    let minecraft = ql.get("depends").and_then(|d| d.as_array()).and_then(|arr| {
        arr.iter().find_map(|d| {
            if d.get("id").and_then(|i| i.as_str()) == Some("minecraft") {
                dep_to_string(d.get("versions"))
            } else {
                None
            }
        })
    });
    Some(ModMeta {
        loader: "quilt".into(),
        mod_id,
        name,
        version: json_str(ql, "version"),
        minecraft,
        sources: None,
    })
}

pub fn parse_mods_toml(text: &str, loader: &str) -> Option<ModMeta> {
    let v: toml::Value = toml::from_str(text).ok()?;
    let first = v.get("mods")?.as_array()?.first()?;
    let mod_id = first.get("modId")?.as_str()?.to_string();
    let get = |k: &str| first.get(k).and_then(|x| x.as_str()).map(String::from);
    let minecraft = v
        .get("dependencies")
        .and_then(|d| d.get(&mod_id))
        .and_then(|deps| deps.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|d| {
                if d.get("modId").and_then(|i| i.as_str()) == Some("minecraft") {
                    d.get("versionRange").and_then(|r| r.as_str()).map(String::from)
                } else {
                    None
                }
            })
        });
    let version = get("version").filter(|v| !v.contains("${"));
    Some(ModMeta {
        loader: loader.into(),
        mod_id,
        name: get("displayName"),
        version,
        minecraft,
        sources: None,
    })
}

/// Manche fabric.mod.json enthalten echte Zeilenumbrüche/Tabs in Strings. Fabric toleriert das,
/// serde_json nicht – deshalb werden Steuerzeichen innerhalb von Strings ersetzt.
fn sanitize_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_str = false;
    let mut escaped = false;
    for ch in text.chars() {
        if in_str {
            if escaped {
                escaped = false;
                out.push(ch);
                continue;
            }
            match ch {
                '\\' => {
                    escaped = true;
                    out.push(ch);
                }
                '"' => {
                    in_str = false;
                    out.push(ch);
                }
                c if (c as u32) < 0x20 => out.push(' '),
                c => out.push(c),
            }
        } else {
            if ch == '"' {
                in_str = true;
            }
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_jar(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            for (name, content) in entries {
                zw.start_file(*name, opts).unwrap();
                zw.write_all(content.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn murmur2_reference_values() {
        // Referenzwerte aus dem npm-Paket "murmurhash" (murmurhash.v2(str, 1))
        assert_eq!(murmur2(b"", 1), 1540447798);
        assert_eq!(murmur2(b"a", 1), 626045324);
        assert_eq!(murmur2(b"ab", 1), 1692487918);
        assert_eq!(murmur2(b"abc", 1), 1621425345);
        assert_eq!(murmur2(b"hello world", 1), 2213174766);
    }

    #[test]
    fn fingerprint_ignores_whitespace() {
        assert_eq!(
            curseforge_fingerprint(b"a b\tc\r\nd"),
            curseforge_fingerprint(b"abcd")
        );
    }

    #[test]
    fn reads_fabric_meta() {
        let jar = make_jar(&[(
            "fabric.mod.json",
            r#"{"schemaVersion":1,"id":"sodium","name":"Sodium","version":"0.6.0","description":"multi
line","depends":{"minecraft":"~1.21.1"}}"#,
        )]);
        let m = read_meta(&jar).unwrap();
        assert_eq!(m.loader, "fabric");
        assert_eq!(m.mod_id, "sodium");
        assert_eq!(m.name.as_deref(), Some("Sodium"));
        assert_eq!(m.minecraft.as_deref(), Some("~1.21.1"));
    }

    #[test]
    fn reads_neoforge_meta() {
        let jar = make_jar(&[(
            "META-INF/neoforge.mods.toml",
            r#"
modLoader="javafml"
loaderVersion="[4,)"
[[mods]]
modId="jei"
displayName="Just Enough Items"
version="${file.jarVersion}"
[[dependencies.jei]]
modId="minecraft"
type="required"
versionRange="[1.21.1,1.21.2)"
"#,
        )]);
        let m = read_meta(&jar).unwrap();
        assert_eq!(m.loader, "neoforge");
        assert_eq!(m.mod_id, "jei");
        assert_eq!(m.version, None);
        assert_eq!(m.minecraft.as_deref(), Some("[1.21.1,1.21.2)"));
    }

    #[test]
    fn reads_quilt_meta() {
        let jar = make_jar(&[(
            "quilt.mod.json",
            r#"{"schema_version":1,"quilt_loader":{"id":"qsl","version":"1.0","metadata":{"name":"QSL"},"depends":[{"id":"minecraft","versions":">=1.20"}]}}"#,
        )]);
        let m = read_meta(&jar).unwrap();
        assert_eq!(m.loader, "quilt");
        assert_eq!(m.name.as_deref(), Some("QSL"));
        assert_eq!(m.minecraft.as_deref(), Some(">=1.20"));
    }

    #[test]
    fn jar_without_meta() {
        let jar = make_jar(&[("readme.txt", "hi")]);
        assert!(read_meta(&jar).is_none());
        assert!(read_meta(b"not a zip").is_none());
    }
}
