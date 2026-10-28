import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorText,
  sourceCandidates,
  type BranchInfo,
  type BuildRunResult,
  type Jdk,
  type OverrideResult,
  type Prepared,
  type ResultRow,
} from "./api";

interface Props {
  row: ResultRow;
  targetVersion: string;
  targetDir: string;
  onPickTargetDir: () => Promise<string | null>;
  onClose: () => void;
  onDone: (message: string) => void;
}

type Step = "source" | "preparing" | "plan" | "building" | "result";
type Tab = "build" | "override";

const MAPPINGS: Record<string, string> = {
  yarn: "Yarn",
  mojang: "Mojang (offiziell)",
  layered: "Geschichtet (z. B. Parchment)",
  none: "Keine (26.x, nicht obfuskiert)",
};

export default function BuildDialog({ row, targetVersion, targetDir, onPickTargetDir, onClose, onDone }: Props) {
  const candidates = useMemo(
    () => sourceCandidates([...row.knownOn.map((k) => k.sourceUrl), row.local?.meta?.sources]),
    [row],
  );
  const canOverride = !!row.local?.meta && ["fabric", "quilt"].includes(row.local.meta.loader);

  const [tab, setTab] = useState<Tab>("build");
  const [step, setStep] = useState<Step>("source");
  const [mode, setMode] = useState<"github" | "local">("github");
  const [url, setUrl] = useState(candidates[0] ?? "");
  const [branches, setBranches] = useState<BranchInfo | null>(null);
  const [branch, setBranch] = useState("");
  const [branchBusy, setBranchBusy] = useState(false);
  const [localPath, setLocalPath] = useState("");
  const [prepared, setPrepared] = useState<Prepared | null>(null);
  const [jdk, setJdk] = useState<Jdk | null | undefined>(undefined);
  const [jdkProgress, setJdkProgress] = useState<[number, number] | null>(null);
  const [consent, setConsent] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<BuildRunResult | null>(null);
  const [installedTo, setInstalledTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showLog, setShowLog] = useState(false);
  const logRef = useRef<HTMLDivElement>(null);

  // Override-Tab
  const [ovCheck, setOvCheck] = useState<string | null | undefined>(undefined);
  const [ovResult, setOvResult] = useState<OverrideResult | null>(null);

  useEffect(() => {
    const a = listen<string>("build-log", (e) =>
      setLog((l) => (l.length > 3000 ? [...l.slice(-2500), e.payload] : [...l, e.payload])),
    );
    const b = listen<[number, number]>("jdk-progress", (e) => setJdkProgress(e.payload));
    return () => {
      a.then((f) => f());
      b.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [log, showLog]);

  useEffect(() => {
    if (mode === "github" && url && !branches) void loadBranches();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (tab !== "override" || ovCheck !== undefined) return;
    api
      .overrideCheck(row.local?.meta?.minecraft ?? null, targetVersion)
      .then(() => setOvCheck(null))
      .catch((e) => setOvCheck(errorText(e)));
  }, [tab, ovCheck, row, targetVersion]);

  async function loadBranches() {
    if (!url.trim()) return;
    setBranchBusy(true);
    setError(null);
    try {
      const b = await api.buildBranches(url.trim(), targetVersion);
      setBranches(b);
      setBranch(b.suggested);
    } catch (e) {
      setBranches(null);
      setError(errorText(e));
    } finally {
      setBranchBusy(false);
    }
  }

  async function pickLocal() {
    const dir = await open({ directory: true, multiple: false, title: "Quellcode-Ordner der Mod" });
    if (typeof dir === "string") setLocalPath(dir);
  }

  async function prepare() {
    setError(null);
    setStep("preparing");
    try {
      const source =
        mode === "github"
          ? ({ kind: "github", url: url.trim(), branch } as const)
          : ({ kind: "local", path: localPath } as const);
      const p = await api.buildPrepare(source, targetVersion, row.name);
      setPrepared(p);
      setConsent(false);
      setStep("plan");
      setJdk(undefined);
      api
        .buildFindJdk(p.template.java)
        .then(setJdk)
        .catch(() => setJdk(null));
    } catch (e) {
      setError(errorText(e));
      setStep("source");
    }
  }

  async function downloadJdk() {
    if (!prepared) return;
    setError(null);
    setJdkProgress([0, 0]);
    try {
      setJdk(await api.buildDownloadJdk(prepared.template.java));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setJdkProgress(null);
    }
  }

  async function build() {
    if (!prepared || !jdk) return;
    setError(null);
    setLog([]);
    setResult(null);
    setInstalledTo(null);
    setStep("building");
    try {
      const r = await api.buildRun(prepared.id, jdk.home, targetVersion);
      setResult(r);
      setShowLog(!r.success && r.errors.length === 0);
      setStep("result");
    } catch (e) {
      setError(errorText(e));
      setStep("plan");
    }
  }

  async function install() {
    if (!result?.outputJar) return;
    let dir = targetDir;
    if (!dir) dir = (await onPickTargetDir()) ?? "";
    if (!dir) return;
    try {
      const to = await api.buildInstall(result.outputJar, dir);
      setInstalledTo(to);
      onDone(`${row.name}: selbst gebaute Version installiert (kompiliert, im Spiel noch nicht getestet).`);
    } catch (e) {
      setError(errorText(e));
    }
  }

  async function installOverride() {
    const local = row.local;
    if (!local?.meta) return;
    let dir = targetDir;
    if (!dir) dir = (await onPickTargetDir()) ?? "";
    if (!dir) return;
    setError(null);
    try {
      const r = await api.overrideInstall(local.path, local.meta.modId, local.meta.minecraft ?? null, targetVersion, dir);
      setOvResult(r);
      onDone(`${row.name}: Original installiert, Versionssperre per Override aufgehoben.`);
    } catch (e) {
      setError(errorText(e));
    }
  }

  const blocked = (prepared?.report.blockers.length ?? 0) > 0;
  const busy = step === "preparing" || step === "building" || jdkProgress !== null;
  const canStart =
    mode === "github" ? !!url.trim() && !!branch && !branchBusy : !!localPath;

  return (
    <div className="modal-backdrop" onMouseDown={() => !busy && onClose()}>
      <div className="modal wide" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <div>
            <h2>{row.name} für {targetVersion}</h2>
            <p className="muted small-text">Experimentell – nur für den eigenen Gebrauch.</p>
          </div>
          <button className="x" onClick={onClose} disabled={busy} aria-label="Schließen">
            ×
          </button>
        </div>

        <div className="tabs">
          <button className={tab === "build" ? "active" : ""} onClick={() => !busy && setTab("build")}>
            Selbst bauen
          </button>
          <button
            className={tab === "override" ? "active" : ""}
            disabled={!canOverride || busy}
            title={canOverride ? "" : "Nur für vorhandene Fabric-/Quilt-JARs"}
            onClick={() => setTab("override")}
          >
            Versionssperre aufheben
          </button>
        </div>

        {error && (
          <div className="banner error">
            <span className="pre">{error}</span>
          </div>
        )}

        {tab === "build" && (
          <div className="dialog-body">
            {(step === "source" || step === "preparing") && (
              <>
                <p className="muted">
                  ModSync lädt den Quellcode, passt nur die Build-Konfiguration an (Gradle, Loader, Fabric API, Java,
                  fabric.mod.json) und baut die Mod neu. Am Code selbst wird nichts geändert.
                </p>
                <div className="segmented two">
                  <button className={mode === "github" ? "active" : ""} onClick={() => setMode("github")}>
                    GitHub
                  </button>
                  <button className={mode === "local" ? "active" : ""} onClick={() => setMode("local")}>
                    Lokaler Ordner
                  </button>
                </div>

                {mode === "github" ? (
                  <>
                    <label className="field">
                      <span>Repository</span>
                      <div className="path-row">
                        <input
                          className="grow-input"
                          placeholder="https://github.com/besitzer/repo"
                          value={url}
                          list="source-candidates"
                          onChange={(e) => {
                            setUrl(e.target.value);
                            setBranches(null);
                            setBranch("");
                          }}
                          onBlur={() => !branches && loadBranches()}
                        />
                        <datalist id="source-candidates">
                          {candidates.map((c) => (
                            <option key={c} value={c} />
                          ))}
                        </datalist>
                        <button className="btn" onClick={loadBranches} disabled={!url.trim() || branchBusy}>
                          {branchBusy ? "Lädt …" : "Branches laden"}
                        </button>
                      </div>
                    </label>
                    {candidates.length === 0 && (
                      <p className="hint warn">
                        Für diese Mod ist kein GitHub-Link hinterlegt. Den Link von der Projektseite einfügen oder den
                        Quellcode als lokalen Ordner wählen.
                      </p>
                    )}
                    {branches && (
                      <label className="field">
                        <span>Branch</span>
                        <select value={branch} onChange={(e) => setBranch(e.target.value)}>
                          {branches.branches.map((b) => (
                            <option key={b} value={b}>
                              {b}
                              {b === branches.defaultBranch ? "  (Standard)" : ""}
                              {b === branches.suggested && b !== branches.defaultBranch ? "  (passt zur Version)" : ""}
                            </option>
                          ))}
                        </select>
                      </label>
                    )}
                  </>
                ) : (
                  <label className="field">
                    <span>Projektordner</span>
                    <div className="path-row">
                      <div className={`path ${localPath ? "" : "unset"}`} title={localPath}>
                        {localPath || "Ordner mit build.gradle auswählen"}
                      </div>
                      <button className="btn" onClick={pickLocal}>
                        Wählen
                      </button>
                    </div>
                    <span className="hint">Der Ordner wird kopiert – das Original bleibt unverändert.</span>
                  </label>
                )}

                <div className="modal-actions">
                  <div className="spacer" />
                  <button className="btn primary" disabled={!canStart || step === "preparing"} onClick={prepare}>
                    {step === "preparing" ? (
                      <>
                        <span className="spinner dark" /> Wird vorbereitet …
                      </>
                    ) : (
                      "Analysieren & anpassen"
                    )}
                  </button>
                </div>
              </>
            )}

            {step === "plan" && prepared && (
              <>
                <div className="facts">
                  <Fact k="Quelle" v={prepared.sourceLabel} />
                  <Fact k="Bisher" v={prepared.info.sourceMinecraft ? `Minecraft ${prepared.info.sourceMinecraft}` : "unbekannt"} />
                  <Fact k="Mappings" v={MAPPINGS[prepared.info.mappings]} />
                  <Fact
                    k="Ziel"
                    v={`MC ${prepared.template.minecraft} · Loader ${prepared.template.loader} · Fabric API ${prepared.template.fabricApi}`}
                  />
                  <Fact
                    k="Werkzeuge"
                    v={`Loom ${prepared.template.loom} · Gradle ${prepared.template.gradleVersion ?? "?"} · Java ${prepared.template.java}`}
                  />
                </div>

                {prepared.report.blockers.map((b) => (
                  <div key={b} className="banner error">
                    <span>{b}</span>
                  </div>
                ))}
                {prepared.report.warnings.map((w) => (
                  <div key={w} className="banner warn">
                    <span>{w}</span>
                  </div>
                ))}

                {!blocked && prepared.report.changes.length > 0 && (
                  <div className="changes">
                    <div className="changes-head">{prepared.report.changes.length} Änderungen</div>
                    {prepared.report.changes.map((c, i) => (
                      <div key={i} className="change">
                        <span className="cfile">{c.file}</span>
                        <span className="cwhat">{c.what}</span>
                        <span className="cdiff">
                          {c.before && <del>{c.before}</del>}
                          {c.after && <ins>{c.after}</ins>}
                        </span>
                      </div>
                    ))}
                  </div>
                )}

                {!blocked && (
                  <>
                    <div className="jdk-line">
                      <span className="label">Java {prepared.template.java}:</span>
                      {jdk === undefined && <span className="muted">wird gesucht …</span>}
                      {jdk && (
                        <span title={jdk.home}>
                          <span className="dot ok" />
                          JDK {jdk.version} gefunden
                        </span>
                      )}
                      {jdk === null && !jdkProgress && (
                        <>
                          <span className="hint warn">Kein passendes JDK installiert.</span>
                          <button className="btn small" onClick={downloadJdk}>
                            JDK {prepared.template.java} herunterladen (Eclipse Temurin, ca. 200 MB)
                          </button>
                        </>
                      )}
                      {jdkProgress && (
                        <span className="muted">
                          <span className="spinner" /> Lade JDK …{" "}
                          {jdkProgress[1] ? `${Math.round((jdkProgress[0] / jdkProgress[1]) * 100)} %` : ""}
                        </span>
                      )}
                    </div>

                    <label className="check consent">
                      <input type="checkbox" checked={consent} onChange={(e) => setConsent(e.target.checked)} />
                      <span>
                        Ein Gradle-Build führt Code aus diesem Projekt auf meinem PC aus. Ich vertraue der Quelle.
                      </span>
                    </label>
                  </>
                )}

                <div className="modal-actions">
                  <button className="btn ghost" onClick={() => api.openFolder(prepared.projectDir).catch(() => {})}>
                    Arbeitskopie öffnen
                  </button>
                  <button className="btn ghost" onClick={() => setStep("source")}>
                    Zurück
                  </button>
                  <div className="spacer" />
                  <button className="btn primary" disabled={blocked || !jdk || !consent} onClick={build}>
                    Bauen
                  </button>
                </div>
              </>
            )}

            {(step === "building" || (step === "result" && showLog)) && (
              <div className="buildlog" ref={logRef}>
                {log.length === 0 ? (
                  <div className="muted">Gradle startet … Der erste Build lädt Minecraft und kann mehrere Minuten dauern.</div>
                ) : (
                  log.map((l, i) => (
                    <div key={i} className={/error|FAILED|Exception/i.test(l) ? "bad" : /BUILD SUCCESSFUL/.test(l) ? "good" : ""}>
                      {l || " "}
                    </div>
                  ))
                )}
              </div>
            )}
            {step === "building" && (
              <div className="modal-actions">
                <span className="muted">
                  <span className="spinner" /> Build läuft …
                </span>
                <div className="spacer" />
                <button className="btn" onClick={() => api.cancel()}>
                  Abbrechen
                </button>
              </div>
            )}

            {step === "result" && result && (
              <>
                {result.success ? (
                  <div className="result ok">
                    <h3>Build erfolgreich</h3>
                    <p>
                      Die Mod wurde für {targetVersion} kompiliert ({result.seconds} s).{" "}
                      <strong>Im Spiel ist sie damit noch nicht getestet</strong> – Mixins können trotzdem beim Start
                      scheitern. Erst in einer Test-Instanz ausprobieren.
                    </p>
                    <div className="mono">{result.outputJar?.split(/[\\/]/).pop()}</div>
                    {installedTo && <p className="hint good">Installiert: {installedTo}</p>}
                  </div>
                ) : result.cancelled ? (
                  <div className="result">
                    <h3>Abgebrochen</h3>
                  </div>
                ) : (
                  <div className="result bad">
                    <h3>Build fehlgeschlagen</h3>
                    {result.hint && <p>{result.hint}</p>}
                    {result.summary && <pre className="summary">{result.summary}</pre>}
                    {result.errors.length > 0 && (
                      <div className="errlist">
                        <div className="changes-head">
                          {result.errors.length} Stelle{result.errors.length === 1 ? "" : "n"} im Code passen nicht mehr
                        </div>
                        {result.errors.slice(0, 40).map((e, i) => (
                          <div key={i} className="err">
                            <span className="cfile">
                              {e.file}
                              {e.line ? `:${e.line}` : ""}
                            </span>
                            <span>{e.message}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
                <div className="modal-actions">
                  <button className="btn ghost" onClick={() => setShowLog(!showLog)}>
                    {showLog ? "Log ausblenden" : "Log anzeigen"}
                  </button>
                  <button className="btn ghost" onClick={() => prepared && api.openFolder(prepared.workDir).catch(() => {})}>
                    Build-Ordner öffnen
                  </button>
                  <div className="spacer" />
                  {!result.success && (
                    <button className="btn" onClick={() => setStep("plan")}>
                      Zurück
                    </button>
                  )}
                  {result.success && !installedTo && (
                    <button className="btn primary" onClick={install}>
                      In Zielordner installieren
                    </button>
                  )}
                  {installedTo && (
                    <button className="btn primary" onClick={onClose}>
                      Fertig
                    </button>
                  )}
                </div>
              </>
            )}
          </div>
        )}

        {tab === "override" && row.local?.meta && (
          <div className="dialog-body">
            <p className="muted">
              Installiert die <strong>unveränderte</strong> JAR und trägt in{" "}
              <code>config/fabric_loader_dependencies.json</code> ein, dass die Mod auch unter {targetVersion} laden
              darf. Das ist eine offizielle Fabric-Funktion – aber ob die Mod dann läuft, ist Glückssache. Klappt nur bei
              kleinen Versionssprüngen und einfachen Mods.
            </p>
            <div className="facts">
              <Fact k="Datei" v={row.local.fileName} />
              <Fact k="Mod-ID" v={row.local.meta.modId} />
              <Fact k="Gebaut für" v={row.local.meta.minecraft ?? "unbekannt"} />
            </div>
            {ovCheck && (
              <div className="banner error">
                <span>{ovCheck}</span>
              </div>
            )}
            {ovResult && (
              <div className="result ok">
                <h3>Eingerichtet</h3>
                <p className="mono small-text">{ovResult.jarPath}</p>
                <p className="mono small-text">{ovResult.configPath}</p>
                <p className="hint">Rückgängig: Eintrag „{row.local.meta.modId}“ aus der Datei löschen und die JAR entfernen.</p>
              </div>
            )}
            <div className="modal-actions">
              <div className="spacer" />
              {!ovResult ? (
                <button className="btn primary" disabled={ovCheck !== null} onClick={installOverride}>
                  Original installieren & Sperre aufheben
                </button>
              ) : (
                <button className="btn primary" onClick={onClose}>
                  Fertig
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function Fact({ k, v }: { k: string; v: string }) {
  return (
    <div className="fact">
      <span>{k}</span>
      <span title={v}>{v}</span>
    </div>
  );
}
