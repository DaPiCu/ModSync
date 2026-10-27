import { useState } from "react";
import { api, errorText } from "./api";

interface Props {
  hasKey: boolean;
  onClose: () => void;
  onChanged: (hasKey: boolean) => void;
}

export default function SettingsDialog({ hasKey, onClose, onChanged }: Props) {
  const [key, setKey] = useState("");
  const [state, setState] = useState<{ kind: "idle" | "busy" | "ok" | "error"; text?: string }>({ kind: "idle" });

  async function test() {
    setState({ kind: "busy", text: "Wird geprüft …" });
    try {
      await api.testCfKey(key || null);
      setState({ kind: "ok", text: "Der Key funktioniert." });
    } catch (e) {
      setState({ kind: "error", text: errorText(e) });
    }
  }

  async function save() {
    if (!key.trim()) return;
    setState({ kind: "busy", text: "Wird geprüft …" });
    try {
      await api.testCfKey(key);
      await api.setCfKey(key);
      onChanged(true);
      setKey("");
      setState({ kind: "ok", text: "Gespeichert im Windows-Anmeldeinformationsspeicher." });
    } catch (e) {
      setState({ kind: "error", text: errorText(e) });
    }
  }

  async function remove() {
    try {
      await api.setCfKey(null);
      onChanged(false);
      setState({ kind: "ok", text: "API-Key entfernt." });
    } catch (e) {
      setState({ kind: "error", text: errorText(e) });
    }
  }

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>CurseForge-API-Key</h2>
          <button className="x" onClick={onClose} aria-label="Schließen">
            ×
          </button>
        </div>
        <p className="muted">
          Modrinth funktioniert ohne Anmeldung. Für CurseForge braucht ModSync einen eigenen, kostenlosen API-Key.
        </p>
        <ol className="howto small">
          <li>
            Auf{" "}
            <button className="link" onClick={() => api.openUrl("https://console.curseforge.com/")}>
              console.curseforge.com
            </button>{" "}
            anmelden.
          </li>
          <li>Unter „API keys“ den Key kopieren (neue Konten müssen ggf. erst freigeschaltet werden).</li>
          <li>Hier einfügen und speichern.</li>
        </ol>

        <div className="field">
          <span>Status</span>
          <div className="status-line">
            <span className={`dot ${hasKey ? "ok" : "off"}`} />
            {hasKey ? "Ein Key ist gespeichert." : "Kein Key gespeichert."}
          </div>
        </div>

        <label className="field">
          <span>{hasKey ? "Neuen Key einfügen" : "API-Key"}</span>
          <input
            type="password"
            autoComplete="off"
            spellCheck={false}
            placeholder="$2a$10$…"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && save()}
          />
        </label>

        {state.text && <div className={`hint ${state.kind === "error" ? "bad" : state.kind === "ok" ? "good" : ""}`}>{state.text}</div>}

        <div className="modal-actions">
          {hasKey && (
            <button className="btn ghost danger" onClick={remove}>
              Key entfernen
            </button>
          )}
          <div className="spacer" />
          <button className="btn" disabled={state.kind === "busy" || (!key && !hasKey)} onClick={test}>
            Testen
          </button>
          <button className="btn primary" disabled={state.kind === "busy" || !key.trim()} onClick={save}>
            Speichern
          </button>
        </div>
      </div>
    </div>
  );
}
