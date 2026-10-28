import { useMemo, useState } from "react";
import { api, formatSize, type InstallResult, type ResultRow, type Status } from "./api";

const STATUS: Record<Status, { label: string; cls: string }> = {
  available: { label: "Verfügbar", cls: "ok" },
  upToDate: { label: "Bereits passend", cls: "neutral" },
  manualDownload: { label: "Manuell laden", cls: "info" },
  notAvailable: { label: "Nicht verfügbar", cls: "warn" },
  notFound: { label: "Nicht gefunden", cls: "muted" },
  error: { label: "Fehler", cls: "bad" },
};

const ORDER: Status[] = ["available", "manualDownload", "notAvailable", "notFound", "upToDate", "error"];

const OUTCOME: Record<InstallResult["outcome"], { label: string; cls: string }> = {
  installed: { label: "Installiert", cls: "ok" },
  skipped: { label: "Übersprungen", cls: "neutral" },
  failed: { label: "Fehlgeschlagen", cls: "bad" },
  cancelled: { label: "Abgebrochen", cls: "muted" },
};

interface Props {
  rows: ResultRow[];
  selected: Set<string>;
  setSelected: (s: Set<string>) => void;
  installResults: Record<string, InstallResult>;
  disabled: boolean;
  onCopyReport: () => void;
  /** Gesetzt, wenn „Selbst bauen“ für das aktuelle Ziel möglich ist (Fabric/Quilt). */
  onBuild?: (row: ResultRow) => void;
}

const selectable = (r: ResultRow) => r.status === "available" && !!r.candidate?.downloadUrl;

export default function Results({ rows, selected, setSelected, installResults, disabled, onCopyReport, onBuild }: Props) {
  const [filter, setFilter] = useState<Status | "all">("all");
  const [query, setQuery] = useState("");

  const counts = useMemo(() => {
    const c: Partial<Record<Status, number>> = {};
    rows.forEach((r) => (c[r.status] = (c[r.status] ?? 0) + 1));
    return c;
  }, [rows]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return rows
      .filter((r) => filter === "all" || r.status === filter)
      .filter(
        (r) =>
          !q ||
          r.name.toLowerCase().includes(q) ||
          (r.local?.fileName.toLowerCase().includes(q) ?? false) ||
          (r.local?.meta?.modId.toLowerCase().includes(q) ?? false),
      )
      .sort((a, b) => {
        const s = ORDER.indexOf(a.status) - ORDER.indexOf(b.status);
        if (s !== 0) return s;
        const d = Number(a.matchKind === "dependency") - Number(b.matchKind === "dependency");
        if (d !== 0) return d;
        return a.name.localeCompare(b.name, "de");
      });
  }, [rows, filter, query]);

  const visibleSelectable = visible.filter(selectable);
  const allVisibleSelected = visibleSelectable.length > 0 && visibleSelectable.every((r) => selected.has(r.key));

  function toggle(key: string) {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    setSelected(next);
  }

  function toggleAll() {
    const next = new Set(selected);
    visibleSelectable.forEach((r) => (allVisibleSelected ? next.delete(r.key) : next.add(r.key)));
    setSelected(next);
  }

  return (
    <div className="results">
      <div className="toolbar">
        <div className="chips">
          <button className={`chip ${filter === "all" ? "active" : ""}`} onClick={() => setFilter("all")}>
            Alle <b>{rows.length}</b>
          </button>
          {ORDER.filter((s) => counts[s]).map((s) => (
            <button
              key={s}
              className={`chip ${STATUS[s].cls} ${filter === s ? "active" : ""}`}
              onClick={() => setFilter(filter === s ? "all" : s)}
            >
              {STATUS[s].label} <b>{counts[s]}</b>
            </button>
          ))}
        </div>
        <div className="toolbar-right">
          <input
            className="search"
            placeholder="Suchen …"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <button className="btn ghost small" onClick={onCopyReport}>
            Bericht kopieren
          </button>
        </div>
      </div>

      <div className="list-head">
        <label className="check">
          <input
            type="checkbox"
            checked={allVisibleSelected}
            disabled={disabled || visibleSelectable.length === 0}
            onChange={toggleAll}
          />
        </label>
        <span>Mod</span>
        <span>Zielversion</span>
        <span>Status</span>
      </div>

      <div className="list">
        {visible.length === 0 && <div className="list-empty">Keine Einträge für diesen Filter.</div>}
        {visible.map((r) => (
          <Row
            key={r.key}
            row={r}
            checked={selected.has(r.key)}
            onToggle={() => toggle(r.key)}
            disabled={disabled}
            result={installResults[r.key]}
            onBuild={onBuild}
          />
        ))}
      </div>
    </div>
  );
}

function Row({
  row,
  checked,
  onToggle,
  disabled,
  result,
  onBuild,
}: {
  row: ResultRow;
  checked: boolean;
  onToggle: () => void;
  disabled: boolean;
  result?: InstallResult;
  onBuild?: (row: ResultRow) => void;
}) {
  const c = row.candidate;
  const st = STATUS[row.status];
  const canSelect = selectable(row);
  const link = c?.projectUrl ?? row.knownOn[0]?.url;
  const icon = c?.iconUrl;

  return (
    <div className={`row ${canSelect && checked ? "selected" : ""}`}>
      <label className="check">
        <input type="checkbox" checked={canSelect && checked} disabled={!canSelect || disabled} onChange={onToggle} />
      </label>

      <div className="mod">
        <div className="avatar">{icon ? <img src={icon} alt="" /> : <span>{row.name.slice(0, 1).toUpperCase()}</span>}</div>
        <div className="mod-text">
          <div className="mod-name">
            {row.name}
            {row.matchKind === "dependency" && <span className="tag dep">Abhängigkeit</span>}
            {row.matchKind === "modId" && (
              <span className="tag warn" title="Nicht per Prüfsumme, sondern über die Mod-ID gefunden – bitte prüfen, ob es die richtige Mod ist.">
                Mod-ID-Treffer
              </span>
            )}
          </div>
          <div className="mod-sub">
            {row.local ? (
              <span title={row.local.path}>{row.local.fileName}</span>
            ) : row.requiredBy.length > 0 ? (
              <span>Benötigt von {row.requiredBy.join(", ")}</span>
            ) : null}
          </div>
        </div>
      </div>

      <div className="version">
        {c ? (
          <>
            <div className="ver-line">
              <span className={`src ${c.source}`}>{c.source === "modrinth" ? "Modrinth" : "CurseForge"}</span>
              <span className="ver" title={c.fileName}>
                {c.versionNumber}
              </span>
              {c.channel !== "release" && <span className="tag warn">{c.channel}</span>}
            </div>
            <div className="mod-sub">
              {c.fileName}
              {c.size ? ` · ${formatSize(c.size)}` : ""}
            </div>
          </>
        ) : (
          <div className="mod-sub">
            {row.knownOn.length > 0
              ? `Bekannt auf ${row.knownOn.map((k) => (k.source === "modrinth" ? "Modrinth" : "CurseForge")).join(" & ")}`
              : "—"}
          </div>
        )}
      </div>

      <div className="status">
        {result ? (
          <span className={`badge ${OUTCOME[result.outcome].cls}`} title={result.message}>
            {OUTCOME[result.outcome].label}
          </span>
        ) : (
          <span className={`badge ${st.cls}`}>{st.label}</span>
        )}
        <div className="status-msg" title={result?.message ?? row.message}>
          {result?.message ?? row.message}
        </div>
        <div className="row-links">
          {link && (
            <button
              className={`link ${row.status === "manualDownload" ? "strong" : ""}`}
              onClick={() => api.openUrl(link)}
            >
              {row.status === "manualDownload" ? "Download-Seite öffnen ↗" : "Projektseite ↗"}
            </button>
          )}
          {onBuild && (row.status === "notAvailable" || row.status === "notFound") && (
            <button className="link build" disabled={disabled} onClick={() => onBuild(row)}>
              Selbst bauen …
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
