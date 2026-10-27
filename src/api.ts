import { invoke } from "@tauri-apps/api/core";

export type Loader = "fabric" | "quilt" | "neoforge" | "forge";
export type Source = "modrinth" | "curseforge";
export type Status =
  | "available"
  | "upToDate"
  | "manualDownload"
  | "notAvailable"
  | "notFound"
  | "error";
export type MatchKind = "hash" | "modId" | "dependency" | "none";

export interface ModMeta {
  loader: string;
  modId: string;
  name?: string | null;
  version?: string | null;
  minecraft?: string | null;
}

export interface LocalMod {
  fileName: string;
  path: string;
  size: number;
  sha1: string;
  sha512: string;
  cfFingerprint: number;
  meta?: ModMeta | null;
}

export interface Target {
  gameVersion: string;
  loader: Loader;
  allowBeta: boolean;
  allowAlpha: boolean;
  includeDependencies: boolean;
  fallbackByModId: boolean;
}

export interface Candidate {
  source: Source;
  projectId: string;
  projectName: string;
  projectUrl: string;
  iconUrl?: string | null;
  versionId: string;
  versionNumber: string;
  channel: string;
  fileName: string;
  downloadUrl?: string | null;
  sha1?: string | null;
  size: number;
  gameVersions: string[];
  loaders: string[];
  published: string;
}

export interface KnownOn {
  source: Source;
  projectId: string;
  projectName: string;
  url: string;
}

export interface ResultRow {
  key: string;
  name: string;
  local?: LocalMod | null;
  requiredBy: string[];
  matchKind: MatchKind;
  status: Status;
  candidate?: Candidate | null;
  knownOn: KnownOn[];
  message: string;
}

export interface CheckReport {
  rows: ResultRow[];
  warnings: string[];
}

export interface GameVersion {
  version: string;
  versionType: string;
}

export interface InstallItem {
  key: string;
  fileName: string;
  url: string;
  sha1?: string | null;
  size: number;
}

export interface InstallResult {
  key: string;
  fileName: string;
  outcome: "installed" | "skipped" | "failed" | "cancelled";
  message: string;
}

export interface Progress {
  phase: "identify" | "versions" | "dependencies" | "install";
  done: number;
  total: number;
  label: string;
}

export interface Settings {
  sourceDir: string;
  targetDir: string;
  gameVersion: string;
  loader: Loader;
  allowBeta: boolean;
  allowAlpha: boolean;
  includeDependencies: boolean;
  fallbackByModId: boolean;
  showSnapshots: boolean;
}

export const api = {
  loadSettings: () => invoke<Settings>("load_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  scanMods: (dir: string) => invoke<LocalMod[]>("scan_mods", { dir }),
  gameVersions: () => invoke<GameVersion[]>("game_versions"),
  checkMods: (mods: LocalMod[], target: Target) =>
    invoke<CheckReport>("check_mods", { mods, target }),
  installMods: (items: InstallItem[], targetDir: string) =>
    invoke<InstallResult[]>("install_mods", { items, targetDir }),
  cancel: () => invoke<void>("cancel_operation"),
  cfKeyStatus: () => invoke<boolean>("cf_key_status"),
  setCfKey: (key: string | null) => invoke<void>("set_cf_key", { key }),
  testCfKey: (key: string | null) => invoke<void>("test_cf_key", { key }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  openFolder: (path: string) => invoke<void>("open_folder", { path }),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

export function formatSize(bytes: number): string {
  if (!bytes) return "";
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
