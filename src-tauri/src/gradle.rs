//! Gradle-Build starten, Ausgabe live weiterreichen, Fehler zusammenfassen und die fertige JAR finden.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

pub const TIMEOUT: Duration = Duration::from_secs(45 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildError {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildOutcome {
    pub success: bool,
    pub cancelled: bool,
    pub jar: Option<String>,
    pub errors: Vec<BuildError>,
    /// Abschnitt „What went wrong“ aus der Gradle-Ausgabe
    pub summary: Option<String>,
    pub log_file: String,
    pub seconds: u64,
}

/// Prozess-ID des laufenden Builds (zum Abbrechen).
pub type ChildSlot = Arc<Mutex<Option<u32>>>;

pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-s", "TERM", "--", &format!("-{pid}")])
            .status();
    }
}

pub async fn run<L: Fn(String) + Send + Sync>(
    project: &Path,
    java_home: &Path,
    log_file: &Path,
    on_line: L,
    cancel: &AtomicBool,
    child_slot: ChildSlot,
) -> Result<BuildOutcome, String> {
    let started = Instant::now();
    let args = ["build", "-x", "test", "--no-daemon", "--console=plain", "--stacktrace"];
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new(project.join("gradlew.bat"));
        c.args(args);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg(project.join("gradlew")).args(args);
        c
    };
    let bin = java_home.join("bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&path));
    cmd.current_dir(project)
        .env("JAVA_HOME", java_home)
        .env("PATH", std::env::join_paths(paths).map_err(|e| e.to_string())?)
        .env_remove("JAVA_TOOL_OPTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Gradle konnte nicht gestartet werden: {e}"))?;
    let pid = child.id();
    *child_slot.lock().unwrap() = pid;

    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let tx2 = tx.clone();
    let r1 = tokio::spawn(async move {
        let mut r = BufReader::new(out).lines();
        while let Ok(Some(l)) = r.next_line().await {
            let _ = tx.send(l);
        }
    });
    let r2 = tokio::spawn(async move {
        let mut r = BufReader::new(err).lines();
        while let Ok(Some(l)) = r.next_line().await {
            let _ = tx2.send(l);
        }
    });

    let mut cancelled = false;
    let mut timed_out = false;
    let status = loop {
        tokio::select! {
            line = rx.recv() => {
                if let Some(l) = line {
                    on_line(l.clone());
                    lines.lock().unwrap().push(l);
                }
            }
            st = child.wait() => break st.ok(),
            _ = tokio::time::sleep(Duration::from_millis(300)) => {
                if !cancelled && (cancel.load(Ordering::SeqCst) || started.elapsed() > TIMEOUT) {
                    if !cancel.load(Ordering::SeqCst) { timed_out = true; }
                    cancelled = true;
                    if let Some(p) = pid { kill_tree(p); }
                    let _ = child.start_kill();
                }
            }
        }
    };
    let _ = r1.await;
    let _ = r2.await;
    while let Ok(l) = rx.try_recv() {
        on_line(l.clone());
        lines.lock().unwrap().push(l);
    }
    *child_slot.lock().unwrap() = None;

    let all = lines.lock().unwrap().clone();
    let mut text = all.join("\n");
    if timed_out {
        text.push_str("\n[ModSync] Zeitlimit überschritten – Build abgebrochen.");
    }
    let _ = std::fs::write(log_file, &text);

    let success = !cancelled && status.map(|s| s.success()).unwrap_or(false);
    let jar = if success { find_jar(project).map(|p| p.to_string_lossy().to_string()) } else { None };
    let (errors, summary) = if success { (vec![], None) } else { parse_errors(&all, project) };
    let summary = if timed_out { Some("Zeitlimit von 45 Minuten überschritten.".into()) } else { summary };
    Ok(BuildOutcome {
        success: success && jar.is_some(),
        cancelled: cancelled && !timed_out,
        errors,
        summary: if success && jar.is_none() {
            Some("Build erfolgreich, aber keine JAR in build/libs gefunden.".into())
        } else {
            summary
        },
        jar,
        log_file: log_file.to_string_lossy().to_string(),
        seconds: started.elapsed().as_secs(),
    })
}

/// Sucht die fertige Mod-JAR in `build/libs` (auch in Unterprojekten).
pub fn find_jar(project: &Path) -> Option<PathBuf> {
    let mut libs = vec![project.join("build").join("libs")];
    if let Ok(entries) = std::fs::read_dir(project) {
        for e in entries.flatten() {
            let p = e.path().join("build").join("libs");
            if p.is_dir() {
                libs.push(p);
            }
        }
    }
    let excluded = ["-sources", "-javadoc", "-dev", "-dev-shadow", "-api", "-slim"];
    let mut best: Option<(u8, std::time::SystemTime, PathBuf)> = None;
    for (idx, dir) in libs.iter().enumerate() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            if !name.ends_with(".jar") {
                continue;
            }
            let stem = name.trim_end_matches(".jar");
            if excluded.iter().any(|x| stem.ends_with(x)) {
                continue;
            }
            // Unterprojekt „fabric“ bevorzugen, sonst Wurzel
            let parent = dir.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase());
            let prio = if parent.as_deref() == Some("fabric") { 0 } else if idx == 0 { 1 } else { 2 };
            let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            let better = match &best {
                None => true,
                Some((bp, bm, _)) => prio < *bp || (prio == *bp && modified > *bm),
            };
            if better {
                best = Some((prio, modified, p));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Fasst Compiler- und Gradle-Fehler verständlich zusammen.
pub fn parse_errors(lines: &[String], project: &Path) -> (Vec<BuildError>, Option<String>) {
    let root = project.to_string_lossy().replace('\\', "/");
    let javac = regex::Regex::new(r"^(.*\.(?:java|kt)):(\d+):\s*(?:error|Fehler):\s*(.*)$").unwrap();
    let kotlin = regex::Regex::new(r"^e:\s*(?:file://)?(.*\.kt):(\d+):\d+\s+(.*)$").unwrap();
    let mut errors: Vec<BuildError> = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim();
        let caps = javac.captures(l).or_else(|| kotlin.captures(l));
        if let Some(c) = caps {
            let file = c[1].replace('\\', "/");
            let file = file.strip_prefix(&root).map(|s| s.trim_start_matches('/').to_string()).unwrap_or(file);
            let mut message = c[3].trim().to_string();
            // javac: „symbol: …“ in den Folgezeilen
            for next in lines.iter().skip(i + 1).take(4) {
                let n = next.trim();
                if n.starts_with("symbol:") || n.starts_with("Symbol:") || n.starts_with("location:") || n.starts_with("Ort:") {
                    message.push_str(" · ");
                    message.push_str(n);
                }
            }
            if !errors.iter().any(|e| e.file.as_deref() == Some(&file) && e.message == message) {
                errors.push(BuildError { file: Some(file), line: c[2].parse().ok(), message });
            }
            if errors.len() >= 100 {
                break;
            }
        }
    }
    let mut summary = None;
    if let Some(start) = lines.iter().position(|l| l.trim() == "* What went wrong:") {
        let mut s = Vec::new();
        for l in lines.iter().skip(start + 1) {
            let t = l.trim_end();
            if t.starts_with("* Try:") || t.starts_with("* Exception is:") || t.starts_with("* Get more help") {
                break;
            }
            if !t.trim().is_empty() {
                s.push(t.trim().to_string());
            }
            if s.len() > 12 {
                break;
            }
        }
        if !s.is_empty() {
            summary = Some(s.join("\n"));
        }
    }
    (errors, summary)
}

/// Übersetzt typische Gradle-Meldungen in einen Hinweis.
pub fn hint(outcome: &BuildOutcome) -> Option<String> {
    let s = outcome.summary.clone().unwrap_or_default().to_lowercase();
    if s.contains("could not resolve") || s.contains("could not find") {
        Some("Eine Abhängigkeit gibt es für die Zielversion nicht (oft eine andere Mod wie Mod Menu). Deren Version in gradle.properties anpassen oder die Abhängigkeit entfernen.".into())
    } else if s.contains("unsupported class file major version") || s.contains("toolchain") {
        Some("Java-Version passt nicht zum Projekt oder zu Gradle.".into())
    } else if s.contains("plugin") && s.contains("not found") {
        Some("Ein Gradle-Plugin wurde nicht gefunden – evtl. fehlt das Fabric-Maven in settings.gradle.".into())
    } else if !outcome.errors.is_empty() {
        Some("Der Code nutzt Minecraft- oder Fabric-Funktionen, die es in der Zielversion so nicht mehr gibt. Das muss im Code von Hand angepasst werden.".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_javac_errors() {
        let lines: Vec<String> = [
            "> Task :compileJava",
            "/w/src/src/main/java/a/B.java:12: error: cannot find symbol",
            "        Foo.bar();",
            "        ^",
            "  symbol:   variable Foo",
            "  location: class B",
            "1 error",
            "",
            "FAILURE: Build failed with an exception.",
            "",
            "* What went wrong:",
            "Execution failed for task ':compileJava'.",
            "> Compilation failed; see the compiler error output for details.",
            "",
            "* Try:",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (errs, summary) = parse_errors(&lines, Path::new("/w/src"));
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].file.as_deref(), Some("src/main/java/a/B.java"));
        assert_eq!(errs[0].line, Some(12));
        assert!(errs[0].message.contains("symbol:   variable Foo"));
        assert!(summary.unwrap().contains("compileJava"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_fake_gradle() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        std::fs::write(
            p.join("gradlew"),
            "echo \"java=$JAVA_HOME\"\necho \"args=$*\"\nmkdir -p build/libs\necho x > build/libs/mod-1.0.jar\necho x > build/libs/mod-1.0-sources.jar\nexit 0\n",
        )
        .unwrap();
        let log = p.join("build.log");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let cancel = AtomicBool::new(false);
        let o = run(p, Path::new("/fake/jdk"), &log, move |l| s2.lock().unwrap().push(l), &cancel, Arc::new(Mutex::new(None)))
            .await
            .unwrap();
        assert!(o.success, "{o:?}");
        assert!(o.jar.unwrap().ends_with("mod-1.0.jar"));
        let seen = seen.lock().unwrap();
        assert!(seen.iter().any(|l| l == "java=/fake/jdk"));
        assert!(seen.iter().any(|l| l.contains("build -x test --no-daemon")));
        assert!(std::fs::read_to_string(log).unwrap().contains("java=/fake/jdk"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_gradle_failure_and_cancel() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        std::fs::write(
            p.join("gradlew"),
            "echo \"$PWD/src/main/java/X.java:3: error: incompatible types\" 1>&2\necho '* What went wrong:'\necho 'Compilation failed'\nexit 1\n",
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let o = run(p, Path::new("/j"), &p.join("l.log"), |_| {}, &cancel, Arc::new(Mutex::new(None))).await.unwrap();
        assert!(!o.success);
        assert_eq!(o.errors.len(), 1);
        assert_eq!(o.errors[0].file.as_deref(), Some("src/main/java/X.java"));
        assert!(hint(&o).unwrap().contains("Code"));

        std::fs::write(p.join("gradlew"), "echo start\nsleep 30\necho never\n").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            c2.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        let o = run(p, Path::new("/j"), &p.join("l.log"), |_| {}, &cancel, Arc::new(Mutex::new(None))).await.unwrap();
        assert!(o.cancelled);
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
