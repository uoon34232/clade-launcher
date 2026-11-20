// Frontend side of the Support actions in Settings. Thin wrappers over fixed backend commands: the
// frontend never names a folder or a file, so it cannot ask the backend to open or export anything
// else. Failures are `BackendError`s whose stable code the UI words in the current language.

import { call } from "@/lib/ipc";

export interface DiagnosticsExport {
  /** The file name of the ZIP (never a path). */
  name: string;
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
};

type FullSupportApi = typeof supportApi;
export type SupportApi = Omit<FullSupportApi, "retiredVaultsCount" | "clearRetiredVaults"> &
  Partial<Pick<FullSupportApi, "retiredVaultsCount" | "clearRetiredVaults">>;
