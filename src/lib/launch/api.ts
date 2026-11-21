// Frontend side of launching a profile. A thin, validating wrapper over Rust commands and events:
// the backend decides everything (account, integrity, Java, the command line); this only starts a
// launch and listens to how the game is doing. No secrets ever pass through here.

import { BackendError, call, listenEvent } from "@/lib/ipc";

export type RunState = "launching" | "playing";

/** How a game ended. `normal` is a clean exit; the others carry a short human message. */
export interface LaunchExit {
  code: number | null;
  kind: "normal" | "crashed" | "failed_to_start";
  /** Stable reason code; the UI words it in the current language. */
  reason: string;
  seconds: number;
}

export type LaunchEvent =
  | { instanceId: string; state: RunState }
  | { instanceId: string; state: "stopped"; exit: LaunchExit | null };

/** Why a launch did not happen: a stable code from the backend (the UI words it), plus the account concerned. */
export class LaunchRefused extends Error {
  readonly code: string;
  readonly accountId: string | null;
  constructor(code: string, message: string, accountId: string | null) {
    super(message);
    this.name = "LaunchRefused";
    this.code = code;
    this.accountId = accountId;
  }
}

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

export function parseLaunchEvent(raw: unknown): LaunchEvent | null {
  if (!isObj(raw) || typeof raw.instanceId !== "string") return null;
  if (raw.state === "launching" || raw.state === "playing")
    return { instanceId: raw.instanceId, state: raw.state };
  if (raw.state !== "stopped") return null;
  const e = raw.exit;
  const kind = isObj(e) ? e.kind : null;
  const exit: LaunchExit | null =
    isObj(e) &&
    (kind === "normal" || kind === "crashed" || kind === "failed_to_start") &&
    typeof e.reason === "string"
      ? {
          code: typeof e.code === "number" ? e.code : null,
          kind,
          reason: e.reason,
          seconds: typeof e.seconds === "number" ? e.seconds : 0,
        }
      : null;
  return { instanceId: raw.instanceId, state: "stopped", exit };
}

export interface GameEmbedStatus {
  active: boolean;
  watching?: boolean;
  instanceId: string | null;
  maximized: boolean;
  closing?: boolean;
}

export const NO_EMBED: GameEmbedStatus = {
  active: false,
  watching: false,
  instanceId: null,
  maximized: false,
  closing: false,
};

export function parseGameEmbedStatus(raw: unknown): GameEmbedStatus {
  if (!isObj(raw)) return NO_EMBED;
  if (raw.active === true) {
    return {
      active: true,
      watching: false,
      instanceId: typeof raw.instanceId === "string" ? raw.instanceId : null,
      maximized: raw.maximized === true,
      closing: raw.closing === true,
    };
  }
  if (raw.watching === true) {
    return {
      active: false,
      watching: true,
      instanceId: typeof raw.instanceId === "string" ? raw.instanceId : null,
      maximized: false,
      closing: false,
    };
  }
  return NO_EMBED;
}

export const launchApi = {
  /** Starts the game. Resolves once the process is running; rejects with `LaunchRefused`. */
  launch: async (id: string): Promise<void> => {
    try {
      await call("launch_profile", { id });
    } catch (e) {
      if (e instanceof BackendError) {
        const data = isObj(e.cause) ? e.cause : {};
        throw new LaunchRefused(
          e.code ?? "unknown",
          e.message,
          typeof data.accountId === "string" ? data.accountId : null,
        );
      }
      throw e;
    }
  },
  /** Games that are starting or running right now (so a reloaded window shows the right state). */
  status: async (): Promise<{ instanceId: string; state: RunState }[]> => {
    const raw = await call("launch_status");
    const out: { instanceId: string; state: RunState }[] = [];
    for (const r of Array.isArray(raw) ? raw : []) {
      if (
        isObj(r) &&
        typeof r.instanceId === "string" &&
        (r.state === "launching" || r.state === "playing")
      ) {
        out.push({ instanceId: r.instanceId, state: r.state });
      }
    }
    return out;
  },
  /** Opens the folder with this profile's game session logs. */
  openLogs: async (id: string): Promise<void> => {
    await call("open_game_logs", { id });
  },
  onState: async (callback: (e: LaunchEvent) => void): Promise<() => void> =>
    listenEvent<unknown>("launch-state", (payload) => {
      const ev = parseLaunchEvent(payload);
      if (ev) callback(ev);
    }),
  embedStatus: async (): Promise<GameEmbedStatus> => {
    const raw = await call("get_game_embed_status");
    return parseGameEmbedStatus(raw);
  },
  setStageBounds: async (bounds: {
    x: number;
    y: number;
    width: number;
    height: number;
    radius: number;
  }): Promise<void> => {
    await call("set_game_stage_bounds", bounds);
  },
  toggleMaximize: async (): Promise<GameEmbedStatus> => {
    const raw = await call("toggle_game_maximize");
    return parseGameEmbedStatus(raw);
  },
  onEmbedState: async (callback: (s: GameEmbedStatus) => void): Promise<() => void> =>
    listenEvent<unknown>("game-embed-state", (payload) => {
      callback(parseGameEmbedStatus(payload));
    }),
};

type EmbedClientMethods = "embedStatus" | "setStageBounds" | "toggleMaximize" | "onEmbedState";
export type LaunchApi = Omit<typeof launchApi, EmbedClientMethods> &
  Partial<Pick<typeof launchApi, EmbedClientMethods>>;
