// The single place where the frontend calls Rust commands. Outside the launcher (plain-browser
// development, unit tests) there is no backend: calls reject with a clear message and nothing
// is invented.

import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "@/lib/settings/uiSettings";

/**
 * A failure reported by the backend: a stable `code` and named `params` (facts such as a profile
 * name or a byte count), from which the UI words the message in the current language
 * (`lib/i18n/errors.ts`). `message` is English detail for developers and logs, never shown.
 */
export class BackendError extends Error {
  readonly code: string | null;
  readonly params: Readonly<Record<string, unknown>>;
  constructor(
    message: string,
    code: string | null = null,
    options?: ErrorOptions & { params?: Record<string, unknown> },
  ) {
    super(message, options);
    this.name = "BackendError";
    this.code = code;
    this.params = options?.params ?? {};
  }
}

export async function call(command: string, args?: Record<string, unknown>): Promise<unknown> {
  if (!inTauri())
    throw new BackendError("The launcher backend is not available here.", "backend_unavailable");
  try {
    return await invoke(command, args);
  } catch (e) {
    if (typeof e === "string") throw new BackendError(e, null, { cause: e });
    if (typeof e === "object" && e !== null && "message" in e && typeof e.message === "string") {
      const code = "code" in e && typeof e.code === "string" ? e.code : null;
      const params =
        "params" in e && typeof e.params === "object" && e.params !== null
          ? (e.params as Record<string, unknown>)
          : undefined;
      throw new BackendError(e.message, code, { cause: e, params });
    }
    throw new BackendError("Something went wrong.", null, { cause: e });
  }
}
