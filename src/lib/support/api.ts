// Frontend side of the Support actions in Settings. Thin wrappers over fixed backend commands: the
// frontend never names a folder or a file, so it cannot ask the backend to open or export anything
// else. Failures are `BackendError`s whose stable code the UI words in the current language.

import { call } from "@/lib/ipc";

export interface DiagnosticsExport {
  /** The file name of the ZIP (never a path). */
  name: string;
}

export interface UpdateCheckResult {
  status: "up_to_date" | "update_available";
  currentVersion: string;
  latestVersion: string | null;
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
    return { status, currentVersion, latestVersion };
  },
  openReleasePage: async (tag?: string | null): Promise<void> => {
    await call("open_release_page", tag ? { tag } : {});
  },
};

type FullSupportApi = typeof supportApi;
export type SupportApi = Omit<
  FullSupportApi,
  "retiredVaultsCount" | "clearRetiredVaults" | "checkForUpdates" | "openReleasePage"
> &
  Partial<
    Pick<
      FullSupportApi,
      "retiredVaultsCount" | "clearRetiredVaults" | "checkForUpdates" | "openReleasePage"
    >
  >;
