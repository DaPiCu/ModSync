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
  sources?: string | null;
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
  sourceUrl?: string | null;
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

// ---- Selbst bauen -----------------------------------------------------------------------

export interface Template {
  minecraft: string;
  loader: string;
  loom: string;
  fabricApi: string;
  pluginId: string;
  mappingsLine?: string | null;
  java: number;
  gradleVersion?: string | null;
  fmjMinecraft: string;
}

export type Mappings = "yarn" | "mojang" | "layered" | "none";

export interface ProjectInfo {
  buildFiles: string[];
  mappings: Mappings;
  architectury: boolean;
  sourceMinecraft?: string | null;
}

export interface Change {
  file: string;
  what: string;
  before: string;
  after: string;
}

export interface PatchReport {
  changes: Change[];
  warnings: string[];
  blockers: string[];
}

export interface Prepared {
  id: string;
  workDir: string;
  projectDir: string;
  sourceLabel: string;
  template: Template;
  info: ProjectInfo;
  report: PatchReport;
  yarn?: string | null;
}

export type BuildSource = { kind: "github"; url: string; branch: string } | { kind: "local"; path: string };

export interface BranchInfo {
  defaultBranch: string;
  branches: string[];
  suggested: string;
}

export interface Jdk {
  home: string;
  major: number;
  version: string;
}

export interface BuildErrorItem {
  file?: string | null;
  line?: number | null;
  message: string;
}

export interface BuildRunResult {
  success: boolean;
  cancelled: boolean;
  jar?: string | null;
  errors: BuildErrorItem[];
  summary?: string | null;
  logFile: string;
  seconds: number;
  outputJar?: string | null;
  hint?: string | null;
}

export interface OverrideResult {
  jarPath: string;
  configPath: string;
  jarCopied: boolean;
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

  buildBranches: (url: string, target: string) => invoke<BranchInfo>("build_branches", { url, target }),
  buildPrepare: (source: BuildSource, targetVersion: string, name: string) =>
    invoke<Prepared>("build_prepare", { request: { source, targetVersion, name } }),
  buildFindJdk: (major: number) => invoke<Jdk | null>("build_find_jdk", { major }),
  buildDownloadJdk: (major: number) => invoke<Jdk>("build_download_jdk", { major }),
  buildRun: (id: string, javaHome: string, target: string) =>
    invoke<BuildRunResult>("build_run", { id, javaHome, target }),
  buildInstall: (jar: string, targetDir: string) => invoke<string>("build_install", { jar, targetDir }),
  overrideCheck: (modMinecraft: string | null, target: string) =>
    invoke<void>("override_check", { modMinecraft, target }),
  overrideInstall: (jar: string, modId: string, modMinecraft: string | null, target: string, targetDir: string) =>
    invoke<OverrideResult>("override_install", { jar, modId, modMinecraft, target, targetDir }),
};

/** Erster GitHub-Link aus den bekannten Quellcode-Angaben einer Zeile. */
export function sourceCandidates(urls: (string | null | undefined)[]): string[] {
  const out: string[] = [];
  for (const u of urls) {
    if (!u) continue;
    const m = u.trim().match(/^https?:\/\/(?:www\.)?github\.com\/([\w.-]+)\/([\w.-]+?)(?:\.git)?(?:\/.*)?$/i);
    if (!m) continue;
    const norm = `https://github.com/${m[1]}/${m[2]}`;
    if (!out.some((x) => x.toLowerCase() === norm.toLowerCase())) out.push(norm);
  }
  return out;
}

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
