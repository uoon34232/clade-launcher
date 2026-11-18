import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));

import {
  parseCatalog,
  parseLoaderOptions,
  parseProfiles,
  profilesApi,
  type NewProfileRequest,
} from "./api";

const tauri = (on: boolean) => {
  if (on) (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  else delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue({});
});
afterEach(() => tauri(false));

describe("parsers validate backend data instead of trusting it", () => {
  it("profiles: keeps valid ones, drops malformed, repairs the selection", () => {
    const s = parseProfiles({
      profiles: [
        {
          id: "a",
          name: "A",
          minecraft: "1.21.4",
          kind: "vanilla",
          loader: null,
          loaderVersion: null,
          ready: false,
          playtimeSeconds: 61,
        },
        {
          id: "b",
          name: "B",
          minecraft: "1.20.1",
          kind: "modded",
          loader: "neoforge",
          loaderVersion: "21.4.1",
          ready: true,
          playtimeSeconds: -5,
        },
        { id: "c", name: "C", minecraft: "1", kind: "weird" },
        { name: "no id" },
        "junk",
        { id: "d", name: "D", minecraft: "1.0", kind: "modded", loader: "notaloader" },
      ],
      selected: "ghost",
      totalPlaytimeSeconds: 99720,
    });
    expect(s.profiles.map((p) => p.id)).toEqual(["a", "b", "d"]);
    expect(s.profiles[1]).toMatchObject({
      loader: "neoforge",
      loaderVersion: "21.4.1",
      ready: true,
      playtimeSeconds: 0,
    });
    expect(s.profiles[2].loader).toBeNull();
    expect(s.profiles[0].memoryMb).toBeNull();
    expect(s.selected).toBe("a");
    expect(s.totalPlaytimeSeconds).toBe(99720);
    expect(s.unreadable).toEqual([]);
    expect(parseProfiles(null)).toEqual({
      profiles: [],
      selected: null,
      totalPlaytimeSeconds: 0,
      unreadable: [],
    });
  });

  it("profiles: validates memoryMb overrides and unreadable instance folder names", () => {
    const mk = (id: string, memoryMb: unknown) => ({
      id,
      name: id,
      minecraft: "1.21.4",
      kind: "vanilla",
      memoryMb,
    });
    const s = parseProfiles({
      profiles: [
        mk("pos", 6144),
        mk("nul", null),
        mk("zero", 0),
        mk("neg", -512),
        mk("nan", Number.NaN),
        mk("inf", Number.POSITIVE_INFINITY),
        mk("str", "4096"),
      ],
      unreadable: ["corrupt-a", "", 42, null, "broken-b"],
    });
    expect(s.profiles.map((p) => [p.id, p.memoryMb])).toEqual([
      ["pos", 6144],
      ["nul", null],
      ["zero", null],
      ["neg", null],
      ["nan", null],
      ["inf", null],
      ["str", null],
    ]);
    expect(s.unreadable).toEqual(["corrupt-a", "broken-b"]);
    expect(parseProfiles({ profiles: [], unreadable: "not-an-array" }).unreadable).toEqual([]);
  });

  it("catalog: only known kinds, and an unknown origin counts as stale", () => {
    const c = parseCatalog({
      entries: [
        { id: "26.3", kind: "release", releaseTime: "2026-09-01T10:00:00+00:00" },
        { id: "x", kind: "hologram", releaseTime: "t" },
        { id: 5, kind: "release" },
        { id: "b1.7.3", kind: "old_beta", releaseTime: "2011" },
      ],
      latestRelease: "26.3",
      latestSnapshot: null,
      fetchedAt: 123,
      origin: "???",
      stale: false,
    });
    expect(c.entries.map((e) => e.id)).toEqual(["26.3", "b1.7.3"]);
    expect(c.origin).toBe("cache_stale");
    expect(c.latestRelease).toBe("26.3");
  });

  it("loader options: never trusts 'ok' without versions and skips unknown loaders", () => {
    const o = parseLoaderOptions([
      {
        loader: "fabric",
        status: "ok",
        versions: [
          { version: "0.19.5", channel: "stable" },
          { version: 3, channel: "stable" },
          { version: "x", channel: "bogus" },
        ],
        recommended: "0.19.5",
        stale: false,
        error: null,
      },
      { loader: "forge", status: "ok", versions: [], recommended: null },
      { loader: "quilt", status: "weird" },
      { loader: "spoon", status: "ok" },
    ]);
    expect(o.map((x) => x.loader)).toEqual(["fabric", "forge", "quilt"]);
    expect(o[0].versions).toEqual([{ version: "0.19.5", channel: "stable" }]);
    expect(o[1].status).toBe("none");
    expect(o[2].status).toBe("unavailable");
  });
});

describe("calls", () => {
  it("never invent data outside the launcher", async () => {
    tauri(false);
    await expect(profilesApi.list()).rejects.toThrow(/not available/);
    await expect(profilesApi.versionCatalog()).rejects.toThrow();
    await expect(profilesApi.rename("survival", "Hardcore")).rejects.toThrow(/not available/);
    await expect(profilesApi.setMemory("survival", 6144)).rejects.toThrow(/not available/);
    await expect(profilesApi.setVersion("survival", "1.21.4")).rejects.toThrow(/not available/);
    await expect(profilesApi.duplicate("survival")).rejects.toThrow(/not available/);
    await expect(profilesApi.openFolder("survival")).rejects.toThrow(/not available/);
    await expect(profilesApi.openInstancesFolder()).rejects.toThrow(/not available/);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("call the matching backend commands with the documented arguments", async () => {
    tauri(true);
    await profilesApi.list();
    await profilesApi.select("my-world");
    await profilesApi.versionCatalog(true);
    await profilesApi.loaderOptions("1.21.4");
    const req: NewProfileRequest = {
      name: "N",
      minecraft: "1.21.4",
      profile: {
        type: "modded",
        loader: "fabric",
        loader_version: { mode: "pinned", version: "0.19.5" },
      },
    };
    await profilesApi.create(req);
    await profilesApi.rename("survival", "Hardcore");
    await profilesApi.setMemory("survival", 6144);
    await profilesApi.setMemory("survival", null);
    await profilesApi.setVersion("survival", "1.21.4");
    await profilesApi.setVersion("survival", "1.21.4", { mode: "pinned", version: "0.19.5" });
    await profilesApi.duplicate("survival");
    await profilesApi.openFolder("survival");
    await profilesApi.openInstancesFolder();
    expect(invoke.mock.calls).toEqual([
      ["list_profiles", undefined],
      ["select_profile", { id: "my-world" }],
      ["get_version_catalog", { force: true }],
      ["get_loader_options", { minecraft: "1.21.4" }],
      ["create_profile", { request: req }],
      ["rename_profile", { id: "survival", name: "Hardcore" }],
      ["set_profile_memory", { id: "survival", mb: 6144 }],
      ["set_profile_memory", { id: "survival", mb: null }],
      ["set_profile_version", { id: "survival", minecraft: "1.21.4", loaderVersion: null }],
      [
        "set_profile_version",
        {
          id: "survival",
          minecraft: "1.21.4",
          loaderVersion: { mode: "pinned", version: "0.19.5" },
        },
      ],
      ["duplicate_profile", { id: "survival" }],
      ["open_profile_folder", { id: "survival" }],
      ["open_instances_folder", undefined],
    ]);
  });

  it("surfaces the backend's message when a command fails", async () => {
    tauri(true);
    invoke.mockRejectedValueOnce("A profile named “Survival” already exists.");
    await expect(
      profilesApi.create({ name: "Survival", minecraft: "1.21.4", profile: { type: "vanilla" } }),
    ).rejects.toThrow(/already exists/);
  });
});

describe("preparation progress", () => {
  it("validates events and never turns an unknown total into a number", async () => {
    const { parsePrepareProgress } = await import("./api");
    const ok = parsePrepareProgress({
      instanceId: "a",
      stage: "assets",
      label: "Downloading assets",
      itemsDone: 3,
      itemsTotal: 10,
      bytesDone: null,
      bytesTotal: null,
      overall: null,
    });
    expect(ok).toMatchObject({
      stage: "assets",
      itemsDone: 3,
      itemsTotal: 10,
      bytesDone: null,
      overall: null,
    });
    expect(
      parsePrepareProgress({ instanceId: "a", stage: "libraries", label: "x", overall: 7 })
        ?.overall,
    ).toBe(1);
    expect(
      parsePrepareProgress({ instanceId: "a", stage: "libraries", label: "x", overall: -1 })
        ?.overall,
    ).toBeNull();
    expect(parsePrepareProgress({ instanceId: "a", stage: "warp", label: "x" })).toBeNull();
    expect(parsePrepareProgress({ stage: "java", label: "x" })).toBeNull();
    expect(parsePrepareProgress("junk")).toBeNull();
  });

  it("prepares through the backend and rejects outside the launcher", async () => {
    await expect(profilesApi.prepare("a")).rejects.toThrow("not available");
    tauri(true);
    invoke.mockResolvedValue({ profiles: [], selected: null, totalPlaytimeSeconds: 0 });
    await profilesApi.prepare("survival");
    expect(invoke).toHaveBeenCalledWith("prepare_profile", { id: "survival", verify: false });
    await profilesApi.prepare("survival", { verify: true });
    expect(invoke).toHaveBeenLastCalledWith("prepare_profile", { id: "survival", verify: true });
  });
});
