// Frontend side of the Support actions in Settings. Thin wrappers over fixed backend commands: the
// frontend never names a folder or a file, so it cannot ask the backend to open or export anything
// else. Failures are `BackendError`s whose stable code the UI words in the current language.

import { listen } from "@tauri-apps/api/event";
import { call } from "@/lib/ipc";
import { inTauri } from "@/lib/settings/uiSettings";

export interface DiagnosticsExport {
  /** The file name of the ZIP (never a path). */
  name: string;
}

export interface UpdateCheckResult {
  status: "up_to_date" | "update_available";
  currentVersion: string;
  latestVersion: string | null;
  canInstall?: boolean;
  assetSize?: number | null;
}

export type UpdateStage =
  "idle" | "downloading" | "verifying" | "extracting" | "ready" | "applying";

export interface UpdateProgress {
  stage: UpdateStage;
  version: string | null;
  bytesDone: number;
  bytesTotal: number;
}

function parseUpdateProgress(raw: unknown): UpdateProgress | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  const stage =
    r.stage === "idle" ||
    r.stage === "downloading" ||
    r.stage === "verifying" ||
    r.stage === "extracting" ||
    r.stage === "ready" ||
    r.stage === "applying"
      ? r.stage
      : null;
  if (!stage) return null;
  const version = typeof r.version === "string" ? r.version : null;
  const bytesDone =
    typeof r.bytesDone === "number" && Number.isFinite(r.bytesDone) && r.bytesDone >= 0
      ? r.bytesDone
      : 0;
  const bytesTotal =
    typeof r.bytesTotal === "number" && Number.isFinite(r.bytesTotal) && r.bytesTotal >= 0
      ? r.bytesTotal
      : 0;
  return { stage, version, bytesDone, bytesTotal };
}

export const supportApi = {
  openCladeFolder: async (): Promise<void> => {
    await call("open_clade_folder");
  },
  openLogsFolder: async (): Promise<void> => {
    await call("open_logs_folder");
  },
  /** Creates the diagnostics ZIP (no passwords, tokens or worlds) and resolves with its file name. */
  exportDiagnostics: async (): Promise<DiagnosticsExport> => {
    const raw = await call("export_diagnostics");
    const name =
      typeof raw === "object" && raw !== null && "name" in raw && typeof raw.name === "string"
        ? raw.name
        : "";
    if (!/^[\w.-]{1,80}$/.test(name)) throw new Error("unexpected diagnostics reply");
    return { name };
  },
  retiredVaultsCount: async (): Promise<number> => {
    const raw = await call("retired_vaults_count");
    return typeof raw === "number" && Number.isFinite(raw) && raw >= 0 ? Math.floor(raw) : 0;
  },
  clearRetiredVaults: async (): Promise<number> => {
    const raw = await call("clear_retired_vaults");
    return typeof raw === "number" && Number.isFinite(raw) && raw >= 0 ? Math.floor(raw) : 0;
  },
  checkForUpdates: async (): Promise<UpdateCheckResult> => {
    const raw = await call("check_for_updates");
    if (typeof raw !== "object" || raw === null) throw new Error("unexpected update reply");
    const r = raw as Record<string, unknown>;
    const status = r.status === "update_available" || r.status === "up_to_date" ? r.status : null;
    const currentVersion = typeof r.currentVersion === "string" ? r.currentVersion : "";
    const latestVersion =
      typeof r.latestVersion === "string"
        ? r.latestVersion
        : r.latestVersion === null
          ? null
          : undefined;
    if (!status || !currentVersion || latestVersion === undefined) {
      throw new Error("unexpected update reply");
    }
    const canInstall = typeof r.canInstall === "boolean" ? r.canInstall : false;
    const assetSize =
      typeof r.assetSize === "number" && Number.isFinite(r.assetSize) && r.assetSize > 0
        ? r.assetSize
        : null;
    return { status, currentVersion, latestVersion, canInstall, assetSize };
  },
  openReleasePage: async (tag?: string | null): Promise<void> => {
    await call("open_release_page", tag ? { tag } : {});
  },
  getUpdateStatus: async (): Promise<UpdateProgress> => {
    const p = parseUpdateProgress(await call("get_update_status"));
    if (!p) throw new Error("unexpected update status reply");
    return p;
  },
  installUpdate: async (tag: string): Promise<UpdateProgress> => {
    const p = parseUpdateProgress(await call("install_update", { tag }));
    if (!p) throw new Error("unexpected update install reply");
    return p;
  },
  cancelUpdate: async (): Promise<void> => {
    await call("cancel_update");
  },
  applyStagedUpdate: async (): Promise<void> => {
    await call("apply_staged_update");
  },
  onUpdateProgress: async (callback: (p: UpdateProgress) => void): Promise<() => void> => {
    if (!inTauri()) return () => undefined;
    return listen<unknown>("update-progress", (e) => {
      const p = parseUpdateProgress(e.payload);
      if (p) callback(p);
    });
  },
};

type FullSupportApi = typeof supportApi;
type OptionalSupportKey =
  | "retiredVaultsCount"
  | "clearRetiredVaults"
  | "checkForUpdates"
  | "openReleasePage"
  | "getUpdateStatus"
  | "installUpdate"
  | "cancelUpdate"
  | "applyStagedUpdate"
  | "onUpdateProgress";
export type SupportApi = Omit<FullSupportApi, OptionalSupportKey> &
  Partial<Pick<FullSupportApi, OptionalSupportKey>>;
