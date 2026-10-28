//! Entwickler-Werkzeug: `cargo run --example patch_dir -- <Projektordner> <Vorlagenordner>`
//! Wendet die Anpassungen auf einen Ordner an (Vorlage = Dateien aus fabric-example-mod).
use modsync_lib::{patcher, template};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, tpl_dir) = (Path::new(&args[1]), Path::new(&args[2]));
    let r = |f: &str| std::fs::read_to_string(tpl_dir.join(f)).ok();
    let tpl = template::parse_template(
        &r("gradle.properties").unwrap(),
        &r("build.gradle").unwrap(),
        r("gradle-wrapper.properties").as_deref(),
        r("fabric.mod.json").as_deref(),
    )
    .unwrap();
    let info = patcher::analyze(dir).unwrap();
    println!("{info:#?}");
    let rep = patcher::apply(dir, &info, &tpl, args.get(3).map(|s| s.as_str())).unwrap();
    println!("{}", serde_json::to_string_pretty(&rep).unwrap());
}
