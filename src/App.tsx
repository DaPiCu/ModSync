import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  type CheckReport,
  type GameVersion,
  type InstallItem,
  type InstallResult,
  type Loader,
  type LocalMod,
  type Progress,
  type Settings,
  type Status,
  type Target,
} from "./api";
import Results from "./Results";
import SettingsDialog from "./SettingsDialog";
import BuildDialog from "./BuildDialog";
import type { ResultRow } from "./api";

const DEFAULT_SETTINGS: Settings = {
  sourceDir: "",
  targetDir: "",
  gameVersion: "",
  loader: "fabric",
  allowBeta: false,
  allowAlpha: false,
  includeDependencies: true,
  fallbackByModId: true,
  showSnapshots: false,
};

const LOADERS: { id: Loader; label: string }[] = [
  { id: "fabric", label: "Fabric" },
  { id: "quilt", label: "Quilt" },
  { id: "neoforge", label: "NeoForge" },
  { id: "forge", label: "Forge" },
];

type Busy = null | "scan" | "check" | "install";

function targetOf(s: Settings): Target {
  return {
    gameVersion: s.gameVersion,
    loader: s.loader,
    allowBeta: s.allowBeta,
    allowAlpha: s.allowAlpha,
    includeDependencies: s.includeDependencies,
    fallbackByModId: s.fallbackByModId,
  };
}

/** Lange Pfade vorne kürzen, damit der relevante Teil (Instanz/mods) sichtbar bleibt. */
function shortPath(p: string, max = 46): string {
  if (p.length <= max) return p;
  const parts = p.split(/[\\/]/);
  let out = parts[parts.length - 1];
  for (let i = parts.length - 2; i > 0; i--) {
    const next = parts[i] + "\\" + out;
    if (next.length + 2 > max) break;
    out = next;
  }
  return "…\\" + out;
}

function samePath(a: string, b: string) {
  const n = (p: string) => p.trim().replace(/[\\/]+$/, "").toLowerCase();
  return a.trim() !== "" && n(a) === n(b);
}

export default function App() {
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  const [loaded, setLoaded] = useState(false);
  const [versions, setVersions] = useState<GameVersion[] | null>(null);
  const [versionsError, setVersionsError] = useState<string | null>(null);
  const [mods, setMods] = useState<LocalMod[] | null>(null);
  const [report, setReport] = useState<CheckReport | null>(null);
  const [reportTarget, setReportTarget] = useState<string>("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [installResults, setInstallResults] = useState<Record<string, InstallResult>>({});
  const [busy, setBusy] = useState<Busy>(null);
  const [progress, setProgress] = useState<Progress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [cfKey, setCfKey] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [buildRow, setBuildRow] = useState<ResultRow | null>(null);
  const saveTimer = useRef<number | undefined>(undefined);

  // ---- Start ----------------------------------------------------------------------------
  useEffect(() => {
    const un = listen<Progress>("progress", (e) => setProgress(e.payload));
    (async () => {
      try {
        const s = await api.loadSettings();
        setSettings({ ...DEFAULT_SETTINGS, ...s });
        if (s.sourceDir) void scan(s.sourceDir);
      } catch (e) {
        setError(errorText(e));
      }
      setLoaded(true);
      api.cfKeyStatus().then(setCfKey).catch(() => setCfKey(false));
      loadVersions();
    })();
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Einstellungen automatisch speichern
  useEffect(() => {
    if (!loaded) return;
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      api.saveSettings(settings).catch(() => {});
    }, 400);
  }, [settings, loaded]);

  function loadVersions() {
    setVersionsError(null);
    api
      .gameVersions()
      .then((v) => {
        setVersions(v);
        setSettings((s) => {
          if (s.gameVersion) return s;
          const latest = v.find((x) => x.versionType === "release");
          return latest ? { ...s, gameVersion: latest.version } : s;
        });
      })
      .catch((e) => setVersionsError(errorText(e)));
  }

  const update = (patch: Partial<Settings>) => setSettings((s) => ({ ...s, ...patch }));

  // ---- Aktionen -------------------------------------------------------------------------
  async function pickFolder(kind: "sourceDir" | "targetDir") {
    const dir = await open({
      directory: true,
      multiple: false,
      defaultPath: settings[kind] || undefined,
      title: kind === "sourceDir" ? "Mods-Ordner auswählen" : "Zielordner auswählen",
    });
    if (typeof dir !== "string") return;
    update({ [kind]: dir } as Partial<Settings>);
    if (kind === "sourceDir") void scan(dir);
  }

  async function pickTargetDir(): Promise<string | null> {
    const dir = await open({ directory: true, multiple: false, title: "Zielordner auswählen" });
    if (typeof dir !== "string") return null;
    update({ targetDir: dir });
    return dir;
  }

  async function scan(dir: string) {
    setBusy("scan");
    setError(null);
    setReport(null);
    setInstallResults({});
    try {
      const found = await api.scanMods(dir);
      setMods(found);
      if (found.length === 0) setNotice("In diesem Ordner liegen keine .jar-Dateien.");
    } catch (e) {
      setMods(null);
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  }

  async function check() {
    if (!mods || mods.length === 0) return;
    setBusy("check");
    setError(null);
    setNotice(null);
    setInstallResults({});
    setProgress(null);
    const t = targetOf(settings);
    try {
      const r = await api.checkMods(mods, t);
      setReport(r);
      setReportTarget(JSON.stringify(t));
      setSelected(
        new Set(
          r.rows.filter((x) => x.status === "available" && x.candidate?.downloadUrl).map((x) => x.key),
        ),
      );
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  const installable = useMemo(
    () =>
      (report?.rows ?? []).filter(
        (r) => selected.has(r.key) && r.status === "available" && r.candidate?.downloadUrl,
      ),
    [report, selected],
  );

  async function install() {
    if (!settings.targetDir) {
      setError("Bitte zuerst einen Zielordner auswählen.");
      return;
    }
    const items: InstallItem[] = installable.map((r) => ({
      key: r.key,
      fileName: r.candidate!.fileName,
      url: r.candidate!.downloadUrl!,
      sha1: r.candidate!.sha1,
      size: r.candidate!.size,
    }));
    setBusy("install");
    setError(null);
    setNotice(null);
    setProgress(null);
    try {
      const res = await api.installMods(items, settings.targetDir);
      const map: Record<string, InstallResult> = { ...installResults };
      res.forEach((x) => (map[x.key] = x));
      setInstallResults(map);
      const ok = res.filter((x) => x.outcome === "installed").length;
      const skipped = res.filter((x) => x.outcome === "skipped").length;
      const failed = res.filter((x) => x.outcome === "failed").length;
      setNotice(
        `${ok} installiert` +
          (skipped ? `, ${skipped} übersprungen` : "") +
          (failed ? `, ${failed} fehlgeschlagen` : "") +
          ".",
      );
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
      setProgress(null);
    }
  }

  function copyReport() {
    if (!report) return;
    const label: Record<Status, string> = {
      available: "Verfügbar",
      upToDate: "Bereits aktuell",
      manualDownload: "Manueller Download",
      notAvailable: "Nicht verfügbar",
      notFound: "Nicht gefunden",
      error: "Fehler",
    };
    const lines = [
      `ModSync-Bericht – Minecraft ${settings.gameVersion} (${settings.loader})`,
      "",
      ...report.rows.map((r) => {
        const link = r.candidate?.projectUrl ?? r.knownOn[0]?.url ?? "";
        const dep = r.matchKind === "dependency" ? " [Abhängigkeit]" : "";
        return `${label[r.status].padEnd(20)} ${r.name}${dep}${link ? "  " + link : ""}`;
      }),
    ];
    navigator.clipboard
      .writeText(lines.join("\n"))
      .then(() => setNotice("Bericht in die Zwischenablage kopiert."))
      .catch(() => setError("Zwischenablage nicht verfügbar."));
  }

  // ---- Abgeleitete Werte ----------------------------------------------------------------
  const versionOptions = useMemo(() => {
    if (!versions) return [];
    const list = versions.filter((v) => settings.showSnapshots || v.versionType === "release");
    if (settings.gameVersion && !list.some((v) => v.version === settings.gameVersion)) {
      list.unshift({ version: settings.gameVersion, versionType: "custom" });
    }
    return list;
  }, [versions, settings.showSnapshots, settings.gameVersion]);

  const checkedTarget = useMemo<Target | null>(() => (reportTarget ? JSON.parse(reportTarget) : null), [reportTarget]);
  const targetChanged = report !== null && reportTarget !== JSON.stringify(targetOf(settings));
  const sameFolder = samePath(settings.sourceDir, settings.targetDir);
  const canCheck = !!mods && mods.length > 0 && !!settings.gameVersion && !busy;

  const progressPct =
    progress && progress.total > 0 ? Math.min(100, (progress.done / progress.total) * 100) : null;
  const progressText = (() => {
    if (busy === "scan") return "Mods werden eingelesen …";
    if (!progress) return busy === "check" ? "Wird vorbereitet …" : "Wird gestartet …";
    switch (progress.phase) {
      case "identify":
        return "Dateien werden erkannt …";
      case "versions":
        return `Suche Versionen: ${progress.done}/${progress.total} – ${progress.label}`;
      case "dependencies":
        return "Abhängigkeiten werden aufgelöst …";
      case "install":
        return progress.done < progress.total
          ? `Lade herunter: ${progress.done + 1}/${progress.total} – ${progress.label}`
          : "Abschließen …";
    }
  })();

  // ---- Oberfläche -----------------------------------------------------------------------
  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <div className="logo" aria-hidden>
            <span />
            <span />
          </div>
          <div>
            <h1>ModSync</h1>
            <p>Mods für eine andere Minecraft-Version finden und installieren</p>
          </div>
        </div>
        <button className="btn ghost" onClick={() => setShowSettings(true)}>
          <span className={`dot ${cfKey ? "ok" : "off"}`} />
          CurseForge {cfKey ? "verbunden" : "nicht eingerichtet"}
        </button>
      </header>

      <div className="layout">
        <aside className="panel side">
          <section className="step">
            <div className="step-head">
              <span className="num">1</span>
              <h2>Vorhandene Mods</h2>
            </div>
            <div className="path-row">
              <div className={`path ${settings.sourceDir ? "" : "unset"}`} title={settings.sourceDir}>
                {settings.sourceDir ? shortPath(settings.sourceDir, 30) : "Kein Ordner ausgewählt"}
              </div>
              <button className="btn" disabled={!!busy} onClick={() => pickFolder("sourceDir")}>
                Wählen
              </button>
            </div>
            {mods && (
              <div className="meta-line">
                <strong>{mods.length}</strong> Mod-Dateien gefunden
                {settings.sourceDir && (
                  <button className="link" disabled={!!busy} onClick={() => scan(settings.sourceDir)}>
                    neu einlesen
                  </button>
                )}
              </div>
            )}
          </section>

          <section className="step">
            <div className="step-head">
              <span className="num">2</span>
              <h2>Zielversion</h2>
            </div>
            <label className="field">
              <span>Minecraft</span>
              {versions ? (
                <select
                  value={settings.gameVersion}
                  onChange={(e) => update({ gameVersion: e.target.value })}
                >
                  {!settings.gameVersion && <option value="">Bitte wählen</option>}
                  {versionOptions.map((v) => (
                    <option key={v.version} value={v.version}>
                      {v.version}
                      {v.versionType !== "release" && v.versionType !== "custom" ? `  (${v.versionType})` : ""}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  placeholder="z. B. 26.2"
                  value={settings.gameVersion}
                  onChange={(e) => update({ gameVersion: e.target.value.trim() })}
                />
              )}
            </label>
            {versionsError && (
              <div className="hint warn">
                Versionsliste nicht geladen: {versionsError}{" "}
                <button className="link" onClick={loadVersions}>
                  erneut versuchen
                </button>
              </div>
            )}
            <label className="check small">
              <input
                type="checkbox"
                checked={settings.showSnapshots}
                onChange={(e) => update({ showSnapshots: e.target.checked })}
              />
              Snapshots und Vorabversionen anzeigen
            </label>

            <div className="field">
              <span>Loader</span>
              <div className="segmented">
                {LOADERS.map((l) => (
                  <button
                    key={l.id}
                    className={settings.loader === l.id ? "active" : ""}
                    onClick={() => update({ loader: l.id })}
                  >
                    {l.label}
                  </button>
                ))}
              </div>
            </div>

            <div className="options">
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.includeDependencies}
                  onChange={(e) => update({ includeDependencies: e.target.checked })}
                />
                Benötigte Abhängigkeiten mit suchen
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.fallbackByModId}
                  onChange={(e) => update({ fallbackByModId: e.target.checked })}
                />
                Unbekannte Dateien über Mod-ID suchen
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.allowBeta}
                  onChange={(e) => update({ allowBeta: e.target.checked })}
                />
                Beta-Versionen erlauben
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={settings.allowAlpha}
                  onChange={(e) => update({ allowAlpha: e.target.checked })}
                />
                Alpha-Versionen erlauben
              </label>
            </div>
          </section>

          <button className="btn primary big" disabled={!canCheck} onClick={check}>
            {busy === "check" ? "Wird geprüft …" : "Verfügbarkeit prüfen"}
          </button>
          {!cfKey && (
            <p className="hint">
              Ohne CurseForge-API-Key wird nur Modrinth durchsucht.{" "}
              <button className="link" onClick={() => setShowSettings(true)}>
                Einrichten
              </button>
            </p>
          )}
        </aside>

        <main className="panel main">
          {error && (
            <div className="banner error">
              <span>{error}</span>
              <button className="x" onClick={() => setError(null)} aria-label="Schließen">
                ×
              </button>
            </div>
          )}
          {notice && !error && (
            <div className="banner info">
              <span>{notice}</span>
              <button className="x" onClick={() => setNotice(null)} aria-label="Schließen">
                ×
              </button>
            </div>
          )}
          {report?.warnings.map((w) => (
            <div key={w} className="banner warn">
              <span>{w}</span>
            </div>
          ))}
          {targetChanged && !busy && (
            <div className="banner warn">
              <span>Die Zielversion oder Optionen wurden geändert – bitte erneut prüfen.</span>
              <button className="btn small" onClick={check} disabled={!canCheck}>
                Erneut prüfen
              </button>
            </div>
          )}

          {busy && (
            <div className="progress-card">
              <div className="progress-text">
                <span className="spinner" />
                <span className="ellipsis">{progressText}</span>
                {busy !== "scan" && (
                  <button className="btn small ghost" onClick={() => api.cancel()}>
                    Abbrechen
                  </button>
                )}
              </div>
              <div className="bar">
                <div
                  className={`fill ${progressPct === null ? "indeterminate" : ""}`}
                  style={progressPct === null ? undefined : { width: `${progressPct}%` }}
                />
              </div>
            </div>
          )}

          {report ? (
            <Results
              rows={report.rows}
              selected={selected}
              setSelected={setSelected}
              installResults={installResults}
              disabled={!!busy}
              onCopyReport={copyReport}
              onBuild={checkedTarget && ["fabric", "quilt"].includes(checkedTarget.loader) ? setBuildRow : undefined}
            />
          ) : (
            !busy && <EmptyState hasMods={!!mods && mods.length > 0} mods={mods} />
          )}

          {report && (
            <footer className="installbar">
              <div className="path-row grow">
                <div className="label">Installieren nach</div>
                <div className={`path ${settings.targetDir ? "" : "unset"}`} title={settings.targetDir}>
                  {settings.targetDir ? shortPath(settings.targetDir, 60) : "Zielordner wählen (z. B. mods-Ordner der neuen Instanz)"}
                </div>
                <button className="btn" disabled={!!busy} onClick={() => pickFolder("targetDir")}>
                  Wählen
                </button>
                {settings.targetDir && (
                  <button className="btn ghost" onClick={() => api.openFolder(settings.targetDir).catch((e) => setError(errorText(e)))}>
                    Öffnen
                  </button>
                )}
              </div>
              {sameFolder && (
                <div className="hint warn">
                  Ziel- und Quellordner sind gleich – alte und neue Versionen würden nebeneinander liegen.
                  Besser einen eigenen Ordner für die neue Version wählen.
                </div>
              )}
              <button
                className="btn primary"
                disabled={!!busy || installable.length === 0 || !settings.targetDir}
                onClick={install}
              >
                {installable.length === 1 ? "1 Mod installieren" : `${installable.length} Mods installieren`}
              </button>
            </footer>
          )}
        </main>
      </div>

      {buildRow && report && (
        <BuildDialog
          row={buildRow}
          targetVersion={checkedTarget?.gameVersion ?? settings.gameVersion}
          targetDir={settings.targetDir}
          onPickTargetDir={pickTargetDir}
          onClose={() => setBuildRow(null)}
          onDone={(m) => setNotice(m)}
        />
      )}

      {showSettings && (
        <SettingsDialog
          hasKey={cfKey}
          onClose={() => setShowSettings(false)}
          onChanged={(v) => setCfKey(v)}
        />
      )}
    </div>
  );
}

function EmptyState({ hasMods, mods }: { hasMods: boolean; mods: LocalMod[] | null }) {
  if (hasMods && mods) {
    return (
      <div className="empty">
        <h3>{mods.length} Mods bereit</h3>
        <p>Zielversion und Loader wählen, dann „Verfügbarkeit prüfen“.</p>
        <ul className="preview">
          {mods.slice(0, 14).map((m) => (
            <li key={m.sha1}>
              <span>{m.meta?.name || m.meta?.modId || m.fileName}</span>
              <small>
                {m.meta?.loader ?? "unbekannt"}
                {m.meta?.minecraft ? ` · MC ${m.meta.minecraft}` : ""}
              </small>
            </li>
          ))}
          {mods.length > 14 && <li className="more">… und {mods.length - 14} weitere</li>}
        </ul>
      </div>
    );
  }
  return (
    <div className="empty">
      <h3>So funktioniert’s</h3>
      <ol className="howto">
        <li>Den mods-Ordner deiner bisherigen Instanz auswählen.</li>
        <li>Zielversion und Loader festlegen.</li>
        <li>Prüfen – jede Datei wird per Prüfsumme auf Modrinth und CurseForge erkannt.</li>
        <li>Gefundene Versionen in einen neuen Ordner installieren.</li>
      </ol>
    </div>
  );
}
