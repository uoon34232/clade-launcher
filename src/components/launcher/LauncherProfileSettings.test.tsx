import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { staleNote } from "@/features/launcher/profileUi";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type { LaunchApi, RunState } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { resetUiSettings, saveUiMode } from "@/lib/settings/uiSettings";
import { readyAccountsClient } from "@/test/testAccounts";
import { LauncherHome } from "./LauncherHome";

vi.mock("@/lib/sound/soundService", () => ({
  playSound: vi.fn(() => true),
  sound: {
    getSettings: () => ({ enabled: true, volume: 0.5 }),
    subscribe: () => () => undefined,
    setSettings: () => undefined,
  },
}));

const profile = (over: Partial<ProfileSummary> = {}): ProfileSummary => ({
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 5400,
  memoryMb: null,
  resolution: null,
  jvmArgs: [],
  ...over,
});

const snap = (
  profiles: ProfileSummary[],
  over: Partial<Omit<ProfilesSnapshot, "profiles">> = {},
): ProfilesSnapshot => ({
  profiles,
  selected: profiles[0]?.id ?? null,
  totalPlaytimeSeconds: profiles.reduce((s, p) => s + p.playtimeSeconds, 0),
  unreadable: [],
  ...over,
});

function stripCopySuffix(name: string): string {
  const trimmed = name.trim();
  const match = /^(.*\S)\s+\((\d+)\)$/.exec(trimmed);
  if (match && Number(match[2]) >= 2) return match[1].trimEnd();
  return trimmed;
}

function slugifyProfile(name: string): string {
  return (
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "profile"
  );
}

function setup(
  initial: ProfilesSnapshot,
  opts: { running?: string; runState?: RunState; signedIn?: boolean } = {},
) {
  let current = initial;
  let rejectPrepare: ((e: unknown) => void) | null = null;
  const api = {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(current)),
    select: vi.fn<ProfilesApi["select"]>((id) => {
      current = { ...current, selected: id };
      return Promise.resolve();
    }),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.resolve({
        entries: [
          { id: "1.21.5", kind: "release", releaseTime: "2025-03-25T10:00:00+00:00" },
          { id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" },
          { id: "25w10a", kind: "snapshot", releaseTime: "2025-03-05T10:00:00+00:00" },
        ],
        latestRelease: "1.21.5",
        latestSnapshot: "25w10a",
        fetchedAt: 1_700_000_000,
        origin: "network",
        stale: false,
        networkError: null,
      }),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() =>
      Promise.resolve([
        {
          loader: "fabric",
          status: "ok",
          versions: [
            { version: "0.16.10", channel: "recommended" },
            { version: "0.16.9", channel: "stable" },
          ],
          recommended: "0.16.10",
          stale: false,
          error: null,
        },
        {
          loader: "forge",
          status: "ok",
          versions: [{ version: "54.1.0", channel: "recommended" }],
          recommended: "54.1.0",
          stale: false,
          error: null,
        },
        {
          loader: "neoforge",
          status: "ok",
          versions: [{ version: "21.4.124", channel: "recommended" }],
          recommended: "21.4.124",
          stale: false,
          error: null,
        },
        {
          loader: "quilt",
          status: "ok",
          versions: [{ version: "0.28.1", channel: "recommended" }],
          recommended: "0.28.1",
          stale: false,
          error: null,
        },
      ]),
    ),
    create: vi.fn<ProfilesApi["create"]>(() => Promise.reject(new Error("not used in this test"))),
    rename: vi.fn<ProfilesApi["rename"]>((id, name) => {
      const trimmed = name.trim();
      current = {
        ...current,
        profiles: current.profiles.map((p) => (p.id === id ? { ...p, name: trimmed } : p)),
      };
      return Promise.resolve(current);
    }),
    setMemory: vi.fn<ProfilesApi["setMemory"]>((id, mb) => {
      current = {
        ...current,
        profiles: current.profiles.map((p) => (p.id === id ? { ...p, memoryMb: mb } : p)),
      };
      return Promise.resolve(current);
    }),
    setResolution: vi.fn<ProfilesApi["setResolution"]>((id, resolution) => {
      current = {
        ...current,
        profiles: current.profiles.map((p) => (p.id === id ? { ...p, resolution } : p)),
      };
      return Promise.resolve(current);
    }),
    setJvmArgs: vi.fn<ProfilesApi["setJvmArgs"]>((id, args) => {
      current = {
        ...current,
        profiles: current.profiles.map((p) => (p.id === id ? { ...p, jvmArgs: [...args] } : p)),
      };
      return Promise.resolve(current);
    }),
    setVersion: vi.fn<ProfilesApi["setVersion"]>((id, minecraft, loaderVersion) => {
      const target = current.profiles.find((p) => p.id === id);
      if (!target) return Promise.reject(new BackendError("missing", "not_found"));
      if (target.kind === "vanilla" && loaderVersion != null) {
        return Promise.reject(
          new BackendError("a Vanilla profile cannot specify a loader version", "storage"),
        );
      }
      const nextLoaderVersion =
        target.kind === "modded"
          ? loaderVersion?.mode === "pinned"
            ? loaderVersion.version
            : null
          : null;
      const unchanged =
        target.minecraft === minecraft &&
        (target.kind === "vanilla" || target.loaderVersion === nextLoaderVersion);
      if (unchanged) {
        return Promise.resolve(current);
      }
      current = {
        ...current,
        profiles: current.profiles.map((p) =>
          p.id === id
            ? {
                ...p,
                minecraft,
                loaderVersion: nextLoaderVersion,
                ready: false,
              }
            : p,
        ),
      };
      return Promise.resolve(current);
    }),
    duplicate: vi.fn<ProfilesApi["duplicate"]>((id) => {
      const src = current.profiles.find((p) => p.id === id);
      if (!src) return Promise.reject(new BackendError("missing", "not_found"));
      const takenNames = new Set(current.profiles.map((p) => p.name.trim().toLowerCase()));
      const unreadable = new Set((current.unreadable ?? []).map((u) => u.toLowerCase()));
      const stem = stripCopySuffix(src.name);
      let copyName = "Profile (copy)";
      for (let n = 2; n <= 9999; n += 1) {
        const suffix = ` (${n})`;
        const head = stem.slice(0, Math.max(0, 64 - suffix.length)).trimEnd();
        const candidate = head ? `${head}${suffix}` : `Profile${suffix}`;
        if (
          !takenNames.has(candidate.toLowerCase()) &&
          !unreadable.has(slugifyProfile(candidate))
        ) {
          copyName = candidate;
          break;
        }
      }
      const baseId = slugifyProfile(copyName);
      const takenIds = new Set([
        ...current.profiles.map((p) => p.id),
        ...(current.unreadable ?? []),
      ]);
      let copyId = baseId;
      if (takenIds.has(copyId)) {
        for (let n = 2; n <= 9999; n += 1) {
          const candidate = `${baseId}-${n}`;
          if (!takenIds.has(candidate)) {
            copyId = candidate;
            break;
          }
        }
      }
      const copy = { ...src, id: copyId, name: copyName, playtimeSeconds: 0 };
      current = {
        ...current,
        profiles: [...current.profiles, copy],
        selected: copyId,
      };
      return Promise.resolve(current);
    }),
    openFolder: vi.fn<ProfilesApi["openFolder"]>(() => Promise.resolve()),
    openInstancesFolder: vi.fn<ProfilesApi["openInstancesFolder"]>(() => Promise.resolve()),
    prepare: vi.fn<ProfilesApi["prepare"]>(
      () =>
        new Promise<ProfilesSnapshot>((_, reject) => {
          rejectPrepare = reject;
        }),
    ),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => {
      rejectPrepare?.(new BackendError("Setup was cancelled.", "cancelled"));
      rejectPrepare = null;
      return Promise.resolve();
    }),
    remove: vi.fn<ProfilesApi["remove"]>((id) => {
      const profiles = current.profiles.filter((p) => p.id !== id);
      current = { ...current, profiles, selected: profiles[0]?.id ?? null };
      return Promise.resolve(current);
    }),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  };

  const launch = {
    launch: vi.fn<LaunchApi["launch"]>(() => Promise.resolve()),
    status: vi.fn<LaunchApi["status"]>(() =>
      Promise.resolve(
        opts.running
          ? [{ instanceId: opts.running, state: opts.runState ?? ("playing" as const) }]
          : [],
      ),
    ),
    openLogs: vi.fn<LaunchApi["openLogs"]>(() => Promise.resolve()),
    onState: vi.fn<LaunchApi["onState"]>(() => Promise.resolve(() => undefined)),
  } satisfies LaunchApi;

  const accounts = readyAccountsClient(
    opts.signedIn === false
      ? { accounts: [], active: null, capabilities: { signIn: true, skin: false, cape: false } }
      : undefined,
  );

  render(<LauncherHome api={api} accountsClient={accounts} launchClient={launch} />);
  return { api, launch, accounts };
}

const flush = async () => {
  await act(async () => {
    for (let i = 0; i < 6; i += 1) {
      await Promise.resolve();
    }
  });
};

const openMenu = async (name: RegExp = /Profile: /) => {
  fireEvent.click(screen.getByRole("button", { name }));
  return screen.findByRole("menu");
};

const openProfileSettings = async (triggerName: RegExp = /Profile: /) => {
  await openMenu(triggerName);
  fireEvent.click(
    screen.getByRole("menuitem", { name: /^(Profile settings…|Настройки профиля…)$/ }),
  );
  return screen.findByRole("dialog");
};

beforeEach(async () => {
  setLocale("en");
  await resetUiSettings();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  if (typeof window.matchMedia !== "function") {
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: (q: string) => ({
        matches: false,
        media: q,
        addEventListener: () => {},
        removeEventListener: () => {},
        addListener: () => {},
        removeListener: () => {},
      }),
    });
  }
});

afterEach(() => cleanup());

describe("Profile settings dialog", () => {
  it("in Simple mode shows details and files only, hiding Memory, Window size, JVM arguments and version filters", async () => {
    setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    expect(d.getByRole("heading", { name: "Survival" })).toBeTruthy();
    expect(d.getByText("Profile")).toBeTruthy();
    expect(d.getByText("Files")).toBeTruthy();
    expect(d.queryByText("Memory")).toBeNull();
    expect(d.queryByText("Window size")).toBeNull();
    expect(d.queryByText("JVM arguments")).toBeNull();

    fireEvent.click(d.getByRole("button", { name: "Change version…" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "1.21.4",
      ),
    );
    expect(d.queryByRole("radiogroup", { name: "Which versions to list" })).toBeNull();
  });

  it("shows header details, Ready vs Needs setup, playtime, default memory, Vanilla vs Modded notes, and actions", async () => {
    await saveUiMode("advanced");
    const modded = profile({
      id: "fabric-pack",
      name: "Fabric Pack",
      minecraft: "1.20.1",
      kind: "modded",
      loader: "fabric",
      loaderVersion: "0.16.9",
      ready: false,
      playtimeSeconds: 0,
    });
    setup(snap([profile(), modded]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    expect(d.getByRole("heading", { name: "Survival" })).toBeTruthy();
    expect(d.getByText("Vanilla 1.21.4")).toBeTruthy();
    expect(d.getByText("Status")).toBeTruthy();
    expect(d.getByText("Ready")).toBeTruthy();
    expect(d.getByText("Playtime")).toBeTruthy();
    expect(d.getByText("1h 30m")).toBeTruthy();
    expect(d.getByRole("radio", { name: "Launcher default" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(d.getByRole("radio", { name: "Custom" }).getAttribute("aria-checked")).toBe("false");
    expect(d.getByText("Follows Settings (4 GB on this computer).")).toBeTruthy();
    expect(
      d.getByText("Worlds, resource packs, screenshots and options live in this profile’s folder."),
    ).toBeTruthy();
    expect(d.getByRole("button", { name: "Open profile folder" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Verify files" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Remove from Clade…" })).toBeTruthy();

    fireEvent.click(d.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    await openMenu();
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Fabric Pack/ }));
    await flush();

    const moddedDialog = await openProfileSettings(/Profile: Fabric Pack/);
    const md = within(moddedDialog);
    expect(md.getByRole("heading", { name: "Fabric Pack" })).toBeTruthy();
    expect(md.getByText("Fabric 1.20.1 · 0.16.9")).toBeTruthy();
    expect(md.getByText("Needs setup")).toBeTruthy();
    expect(md.getByText("0m")).toBeTruthy();
    expect(
      md.getByText("Place compatible mods in the mods folder inside this profile’s folder."),
    ).toBeTruthy();
  });

  it("renames a profile (including case-only), updates header and dock immediately, and validates invalid or duplicate names", async () => {
    const { api } = setup(
      snap([
        profile({ id: "survival", name: "survival" }),
        profile({ id: "creative", name: "Creative" }),
      ]),
    );
    await flush();

    const dialog = await openProfileSettings(/Profile: survival/);
    const d = within(dialog);
    const input = d.getByLabelText<HTMLInputElement>("Name");
    const save = () => d.getByRole<HTMLButtonElement>("button", { name: "Save" });

    expect(input.value).toBe("survival");
    expect(save().disabled).toBe(true);

    // Surrounding whitespace without an actual change stays unchanged.
    fireEvent.change(input, { target: { value: "  survival  " } });
    expect(save().disabled).toBe(true);

    // Empty name
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.blur(input);
    expect(d.getByRole("alert").textContent).toBe("Give the profile a name.");
    expect(save().disabled).toBe(true);

    // Over 64 characters
    fireEvent.change(input, { target: { value: "x".repeat(65) } });
    expect(d.getByRole("alert").textContent).toBe("Keep the name under 64 characters.");
    expect(save().disabled).toBe(true);

    // Control characters
    fireEvent.change(input, { target: { value: "Bad\u0007Name" } });
    expect(d.getByRole("alert").textContent).toBe("The name can’t contain control characters.");
    expect(save().disabled).toBe(true);

    // Duplicate of another profile (case-insensitive)
    fireEvent.change(input, { target: { value: "  creative " } });
    expect(d.getByRole("alert").textContent).toBe("You already have a profile with this name.");
    expect(save().disabled).toBe(true);

    // Case-only rename of THIS profile is allowed!
    fireEvent.change(input, { target: { value: "Survival" } });
    expect(d.queryByRole("alert")).toBeNull();
    expect(save().disabled).toBe(false);
    fireEvent.click(save());

    await waitFor(() => expect(api.rename).toHaveBeenCalledWith("survival", "Survival"));
    await waitFor(() => expect(d.getByRole("heading", { name: "Survival" })).toBeTruthy());
    expect(save().disabled).toBe(true);
    expect(
      screen.getByRole("button", { name: "Profile: Survival. Change profile", hidden: true }),
    ).toBeTruthy();

    // Full rename to a brand-new name
    fireEvent.change(input, { target: { value: "  Weekend World  " } });
    expect(save().disabled).toBe(false);
    fireEvent.click(save());

    await waitFor(() => expect(api.rename).toHaveBeenCalledWith("survival", "Weekend World"));
    await waitFor(() => expect(d.getByRole("heading", { name: "Weekend World" })).toBeTruthy());
    fireEvent.click(d.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(
      screen.getByRole("button", { name: "Profile: Weekend World. Change profile" }),
    ).toBeTruthy();
  });

  it("surfaces backend rename errors (duplicate_name and already_running) inline", async () => {
    const { api } = setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    const input = d.getByLabelText("Name");

    api.rename.mockRejectedValueOnce(
      new BackendError("duplicate", "duplicate_name", { params: { name: "Taken" } }),
    );
    fireEvent.change(input, { target: { value: "Taken" } });
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "A profile named “Taken” already exists.",
    );

    api.rename.mockRejectedValueOnce(new BackendError("busy", "already_running"));
    fireEvent.change(input, { target: { value: "Another" } });
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "This profile is starting or running. Close the game first.",
    );
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("switches memory between Launcher default and Custom, commits 256 MiB slider steps, clamps saved values, and words invalid_memory", async () => {
    await saveUiMode("advanced");
    const { api } = setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    fireEvent.click(d.getByRole("radio", { name: "Custom" }));
    await waitFor(() => expect(api.setMemory).toHaveBeenCalledWith("survival", 4096));

    const slider = await d.findByRole<HTMLInputElement>("slider", {
      name: "Memory for Minecraft",
    });
    expect(slider.step).toBe("256");
    expect(slider.value).toBe("4096");
    expect(slider.getAttribute("aria-valuetext")).toBe("4 GB");

    // Pointer drag + release
    fireEvent.change(slider, { target: { value: "6144" } });
    fireEvent.pointerUp(slider);
    await waitFor(() => expect(api.setMemory).toHaveBeenCalledWith("survival", 6144));
    expect(slider.getAttribute("aria-valuetext")).toBe("6 GB");

    // Keyboard adjustment + release
    fireEvent.change(slider, { target: { value: "4608" } });
    fireEvent.keyUp(slider);
    await waitFor(() => expect(api.setMemory).toHaveBeenCalledWith("survival", 4608));
    expect(slider.getAttribute("aria-valuetext")).toBe("4.5 GB");

    // Blur commit
    fireEvent.change(slider, { target: { value: "5120" } });
    fireEvent.blur(slider);
    await waitFor(() => expect(api.setMemory).toHaveBeenCalledWith("survival", 5120));
    expect(slider.getAttribute("aria-valuetext")).toBe("5 GB");

    // Backend invalid_memory refusal formats {min} and {max} in GB
    api.setMemory.mockRejectedValueOnce(
      new BackendError("unsafe", "invalid_memory", { params: { min: 1024, max: 12288 } }),
    );
    fireEvent.change(slider, { target: { value: "8192" } });
    fireEvent.pointerUp(slider);
    expect((await d.findByRole("alert")).textContent).toBe(
      "That amount of memory isn’t safe for this computer. Choose between 1 GB and 12 GB.",
    );

    // Back to Launcher default
    fireEvent.click(d.getByRole("radio", { name: "Launcher default" }));
    await waitFor(() => expect(api.setMemory).toHaveBeenCalledWith("survival", null));
    expect(await d.findByText("Follows Settings (4 GB on this computer).")).toBeTruthy();
    expect(d.queryByRole("slider", { name: "Memory for Minecraft" })).toBeNull();
    cleanup();

    // Saved out-of-range memoryMb from another PC is clamped to this computer's [min, max]
    setup(snap([profile({ id: "high", name: "High", memoryMb: 32768 })]));
    await flush();
    const highDialog = await openProfileSettings(/Profile: High/);
    const highSlider = within(highDialog).getByRole<HTMLInputElement>("slider", {
      name: "Memory for Minecraft",
    });
    expect(highSlider.value).toBe("16384");
    expect(highSlider.getAttribute("aria-valuetext")).toBe("16 GB");
    cleanup();

    setup(snap([profile({ id: "low", name: "Low", memoryMb: 512 })]));
    await flush();
    const lowDialog = await openProfileSettings(/Profile: Low/);
    const lowSlider = within(lowDialog).getByRole<HTMLInputElement>("slider", {
      name: "Memory for Minecraft",
    });
    expect(lowSlider.value).toBe("1024");
    expect(lowSlider.getAttribute("aria-valuetext")).toBe("1 GB");
  });

  it("configures Window size and JVM arguments in Advanced mode and surfaces validation errors inline", async () => {
    await saveUiMode("advanced");
    const { api } = setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    // Window size starts at Game default
    expect(d.getByRole("radio", { name: "Game default" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(d.getByText("Starts with Minecraft’s standard window size.")).toBeTruthy();

    // Toggle failure surfaces backend error and stays on Game default
    api.setResolution.mockRejectedValueOnce(new BackendError("busy", "already_running"));
    fireEvent.click(d.getByRole("radio", { name: "Fixed size" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "This profile is starting or running. Close the game first.",
    );
    expect(d.getByRole("radio", { name: "Game default" }).getAttribute("aria-checked")).toBe(
      "true",
    );

    // Switching to Fixed size calls setResolution with 1280x720 and leaves Apply size disabled
    fireEvent.click(d.getByRole("radio", { name: "Fixed size" }));
    await waitFor(() =>
      expect(api.setResolution).toHaveBeenCalledWith("survival", { width: 1280, height: 720 }),
    );
    const widthInput = await d.findByLabelText<HTMLInputElement>("Window width");
    const heightInput = d.getByLabelText<HTMLInputElement>("Window height");
    const applySize = d.getByRole<HTMLButtonElement>("button", { name: "Apply size" });
    expect(widthInput.value).toBe("1280");
    expect(heightInput.value).toBe("720");
    expect(applySize.disabled).toBe(true);

    // Out-of-range, max-width, height, and non-numeric dimensions are caught inline without calling the backend
    const resCallsAfterToggle = api.setResolution.mock.calls.length;
    for (const [w, h] of [
      ["100", "720"],
      ["8000", "720"],
      ["1280", "100"],
      ["1280", "5000"],
      ["abc", "720"],
    ]) {
      fireEvent.change(widthInput, { target: { value: w } });
      fireEvent.change(heightInput, { target: { value: h } });
      expect(applySize.disabled).toBe(false);
      fireEvent.click(applySize);
      expect((await d.findByRole("alert")).textContent).toBe(
        "Choose a window size between 320×240 and 7680×4320.",
      );
      expect(api.setResolution).toHaveBeenCalledTimes(resCallsAfterToggle);
    }

    // Apply size backend failure surfaces inline error
    api.setResolution.mockRejectedValueOnce(
      new BackendError("bad resolution", "invalid_resolution", {
        params: { minWidth: 320, maxWidth: 7680, minHeight: 240, maxHeight: 4320 },
      }),
    );
    fireEvent.change(widthInput, { target: { value: "1600" } });
    fireEvent.change(heightInput, { target: { value: "900" } });
    fireEvent.click(applySize);
    expect((await d.findByRole("alert")).textContent).toBe(
      "Choose a window size between 320×240 and 7680×4320.",
    );

    // Valid dimensions commit and disable Apply size again
    fireEvent.change(widthInput, { target: { value: "1920" } });
    fireEvent.change(heightInput, { target: { value: "1080" } });
    fireEvent.click(applySize);
    await waitFor(() =>
      expect(api.setResolution).toHaveBeenCalledWith("survival", { width: 1920, height: 1080 }),
    );
    await waitFor(() => expect(applySize.disabled).toBe(true));

    // Back to Game default clears resolution
    fireEvent.click(d.getByRole("radio", { name: "Game default" }));
    await waitFor(() => expect(api.setResolution).toHaveBeenCalledWith("survival", null));
    expect(await d.findByText("Starts with Minecraft’s standard window size.")).toBeTruthy();

    // JVM arguments: commit valid flags, handle backend refusal, and clear flags
    const jvmInput = d.getByLabelText<HTMLInputElement>("JVM arguments");
    const applyFlags = d.getByRole<HTMLButtonElement>("button", { name: "Apply flags" });
    expect(applyFlags.disabled).toBe(true);

    fireEvent.change(jvmInput, {
      target: { value: " -XX:+UseStringDeduplication  -Dfml.readTimeout=120 " },
    });
    expect(applyFlags.disabled).toBe(false);
    fireEvent.click(applyFlags);
    await waitFor(() =>
      expect(api.setJvmArgs).toHaveBeenCalledWith("survival", [
        "-XX:+UseStringDeduplication",
        "-Dfml.readTimeout=120",
      ]),
    );
    expect(applyFlags.disabled).toBe(true);

    api.setJvmArgs.mockRejectedValueOnce(new BackendError("forbidden", "invalid_jvm_args"));
    fireEvent.change(jvmInput, { target: { value: "-Xmx4G" } });
    fireEvent.click(applyFlags);
    expect((await d.findByRole("alert")).textContent).toMatch(/Only safe JVM flags/);

    fireEvent.change(jvmInput, { target: { value: "   " } });
    fireEvent.click(applyFlags);
    await waitFor(() => expect(api.setJvmArgs).toHaveBeenCalledWith("survival", []));
  });

  it("opens the profile folder from menu and dialog (surfacing open_failed), and delegates Verify files and Remove from Clade…", async () => {
    const { api } = setup(snap([profile(), profile({ id: "b", name: "Beta" })]));
    await flush();

    // Direct menu item
    await openMenu();
    fireEvent.click(screen.getByRole("menuitem", { name: "Open profile folder" }));
    await waitFor(() => expect(api.openFolder).toHaveBeenCalledWith("survival"));
    expect(screen.queryByRole("dialog")).toBeNull();

    // Inside ProfileDialog + open_failed error
    const dialog = await openProfileSettings();
    const d = within(dialog);
    fireEvent.click(d.getByRole("button", { name: "Open profile folder" }));
    await waitFor(() => expect(api.openFolder).toHaveBeenCalledTimes(2));

    api.openFolder.mockRejectedValueOnce(new BackendError("explorer failed", "open_failed"));
    fireEvent.click(d.getByRole("button", { name: "Open profile folder" }));
    expect((await d.findByRole("alert")).textContent).toBe("Windows couldn’t open that folder.");

    // Duplicate failure surfaces inline in the Files section
    api.duplicate.mockRejectedValueOnce(new BackendError("disk full", "duplicate_failed"));
    fireEvent.click(d.getByRole("button", { name: "Duplicate" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "Could not duplicate the profile. Check available disk space and close anything using its folder.",
    );

    // Verify files delegates to PrepareDialog ({ verify: true })
    fireEvent.click(d.getByRole("button", { name: "Verify files" }));
    await waitFor(() => expect(api.prepare).toHaveBeenCalledWith("survival", { verify: true }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    // Remove from Clade… delegates to RemoveProfileDialog
    const reopened = await openProfileSettings();
    fireEvent.click(within(reopened).getByRole("button", { name: "Remove from Clade…" }));
    const confirmDialog = await screen.findByRole("dialog", {
      name: "Remove “Survival” from Clade?",
    });
    expect(confirmDialog.textContent).toContain("Nothing will be permanently deleted.");
    fireEvent.click(within(confirmDialog).getByRole("button", { name: "Remove from Clade" }));
    await waitFor(() => expect(api.remove).toHaveBeenCalledWith("survival"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("disables Verify files when signed out while keeping profile edits active", async () => {
    setup(snap([profile()]), { signedIn: false });
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Verify files" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Remove from Clade…" }).disabled).toBe(
      false,
    );
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Duplicate" }).disabled).toBe(false);
  });

  it("blocks backdrop close when dirty and locks Escape and actions while saving", async () => {
    const { api } = setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    const input = d.getByLabelText<HTMLInputElement>("Name");

    // Dirty dialog stays open on backdrop press
    fireEvent.change(input, { target: { value: "Survival Plus" } });
    const overlay = document.querySelector<HTMLElement>('[data-slot="dialog-overlay"]');
    expect(overlay).toBeTruthy();
    if (overlay) {
      fireEvent.pointerDown(overlay, { button: 0 });
      fireEvent.mouseDown(overlay);
      fireEvent.click(overlay);
    }
    expect(screen.getByRole("dialog")).toBeTruthy();

    // Pending save locks actions and Escape
    let resolveRename!: (snap: ProfilesSnapshot) => void;
    api.rename.mockImplementationOnce(
      () =>
        new Promise<ProfilesSnapshot>((resolve) => {
          resolveRename = resolve;
        }),
    );
    fireEvent.click(d.getByRole("button", { name: "Save" }));

    expect(d.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Duplicate" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Remove from Clade…" }).disabled).toBe(
      true,
    );
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.getByRole("dialog")).toBeTruthy();

    resolveRename(snap([profile({ name: "Survival Plus" })]));
    await waitFor(() =>
      expect(d.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(false),
    );

    // Clean dialog closes on backdrop press
    const cleanOverlay = document.querySelector<HTMLElement>('[data-slot="dialog-overlay"]');
    if (cleanOverlay) {
      fireEvent.pointerDown(cleanOverlay, { button: 0 });
      fireEvent.mouseDown(cleanOverlay);
      fireEvent.click(cleanOverlay);
    }
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("disables edits, verification and removal while launching or playing, keeping Open profile folder and Done active", async () => {
    await saveUiMode("advanced");
    const { api } = setup(
      snap([profile({ memoryMb: 6144, resolution: { width: 1280, height: 720 } })]),
      {
        running: "survival",
        runState: "launching",
      },
    );
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    expect(d.getByLabelText<HTMLInputElement>("Name").disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Change version…" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Launcher default" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Custom" }).disabled).toBe(true);
    expect(d.getByRole<HTMLInputElement>("slider", { name: "Memory for Minecraft" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Game default" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Fixed size" }).disabled).toBe(true);
    expect(d.getByLabelText<HTMLInputElement>("Window width").disabled).toBe(true);
    expect(d.getByLabelText<HTMLInputElement>("Window height").disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Apply size" }).disabled).toBe(true);
    expect(d.getByLabelText<HTMLInputElement>("JVM arguments").disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Apply flags" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Verify files" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Remove from Clade…" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Duplicate" }).disabled).toBe(true);

    const openFolder = d.getByRole<HTMLButtonElement>("button", { name: "Open profile folder" });
    expect(openFolder.disabled).toBe(false);
    fireEvent.click(openFolder);
    await waitFor(() => expect(api.openFolder).toHaveBeenCalledWith("survival"));

    const done = d.getByRole<HTMLButtonElement>("button", { name: "Done" });
    expect(done.disabled).toBe(false);
    fireEvent.click(done);
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("unreadable profile folders recovery strip", () => {
  it("is hidden when unreadable is empty, and shows singular or plural wording with Open profiles folder when non-empty", async () => {
    setup(snap([profile()], { unreadable: [] }));
    await flush();
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("button", { name: "Open profiles folder" })).toBeNull();
    cleanup();

    const single = setup(snap([profile()], { unreadable: ["broken-world"] }));
    await flush();
    const oneBanner = await screen.findByRole("status");
    expect(oneBanner.textContent).toContain(
      "1 profile folder couldn’t be read (broken-world). Its files were left untouched.",
    );
    single.api.openInstancesFolder.mockRejectedValueOnce(
      new BackendError("explorer failed", "open_failed"),
    );
    fireEvent.click(within(oneBanner).getByRole("button", { name: "Open profiles folder" }));
    expect((await within(oneBanner).findByRole("alert")).textContent).toBe(
      "Windows couldn’t open that folder.",
    );

    fireEvent.click(within(oneBanner).getByRole("button", { name: "Open profiles folder" }));
    await waitFor(() => expect(single.api.openInstancesFolder).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(within(oneBanner).queryByRole("alert")).toBeNull());
    cleanup();

    setup(snap([profile()], { unreadable: ["broken-world", "corrupt-pack"] }));
    await flush();
    expect((await screen.findByRole("status")).textContent).toContain(
      "2 profile folders couldn’t be read (broken-world, corrupt-pack). Their files were left untouched.",
    );
  });
});

describe("Profile settings and unreadable notice in Russian", () => {
  it("renders ProfileDialog, errors and the unreadable folders strip in Russian", async () => {
    setLocale("ru");
    await saveUiMode("advanced");
    const { api } = setup(snap([profile()], { unreadable: ["broken-world", "corrupt-pack"] }));
    await flush();

    const banner = screen.getByRole("status");
    expect(banner.textContent).toContain(
      "Не удалось прочитать 2 папки профилей (broken-world, corrupt-pack). Файлы не затронуты.",
    );
    fireEvent.click(within(banner).getByRole("button", { name: "Открыть папку профилей" }));
    await waitFor(() => expect(api.openInstancesFolder).toHaveBeenCalledTimes(1));

    const dialog = await openProfileSettings(/Профиль: Survival/);
    const d = within(dialog);
    expect(d.getByText("Профиль")).toBeTruthy();
    expect(d.getByLabelText("Название")).toBeTruthy();
    expect(d.getByRole("button", { name: "Сохранить" })).toBeTruthy();
    expect(d.getByText("Статус")).toBeTruthy();
    expect(d.getByText("Готов")).toBeTruthy();
    expect(d.getByText("Время в игре")).toBeTruthy();
    expect(d.getByText("1 ч 30 мин")).toBeTruthy();
    expect(d.getByText("Память")).toBeTruthy();
    expect(d.getByRole("radio", { name: "По умолчанию" })).toBeTruthy();
    expect(d.getByRole("radio", { name: "Своя" })).toBeTruthy();
    expect(d.getByText("Как в настройках лаунчера (4 ГБ на этом компьютере).")).toBeTruthy();
    expect(d.getByText("Размер окна")).toBeTruthy();
    expect(d.getByRole("radio", { name: "Стандартный" })).toBeTruthy();
    expect(d.getByRole("radio", { name: "Заданный" })).toBeTruthy();
    expect(d.getByLabelText("Аргументы JVM")).toBeTruthy();
    expect(d.getByRole("button", { name: "Применить флаги" })).toBeTruthy();
    expect(d.getByText("Файлы")).toBeTruthy();
    expect(
      d.getByText("Миры, ресурспаки, скриншоты и настройки хранятся в папке профиля."),
    ).toBeTruthy();
    expect(d.getByRole("button", { name: "Открыть папку профиля" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Проверить файлы" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Убрать из Clade…" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Готово" })).toBeTruthy();

    api.setMemory.mockRejectedValueOnce(
      new BackendError("unsafe", "invalid_memory", { params: { min: 1024, max: 12288 } }),
    );
    fireEvent.click(d.getByRole("radio", { name: "Своя" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "Такой объём памяти небезопасен для этого компьютера. Выбери от 1 ГБ до 12 ГБ.",
    );

    api.openFolder.mockRejectedValueOnce(new BackendError("nope", "open_failed"));
    fireEvent.click(d.getByRole("button", { name: "Открыть папку профиля" }));
    await waitFor(() =>
      expect(
        d
          .getAllByRole("alert")
          .some((a) => a.textContent === "Не удалось открыть эту папку в Windows."),
      ).toBe(true),
    );
  });

  it("changes a profile's Minecraft version in place and duplicates a profile onto the new copy", async () => {
    await saveUiMode("advanced");
    const { api } = setup(snap([profile()]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    fireEvent.click(d.getByRole("button", { name: "Change version…" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "1.21.4",
      ),
    );
    fireEvent.click(d.getByRole("radio", { name: "Snapshots" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "25w10a",
      ),
    );
    fireEvent.click(d.getByRole("button", { name: "Save version" }));

    await waitFor(() => expect(api.setVersion).toHaveBeenCalledWith("survival", "25w10a", null));
    expect(await d.findByText("Needs setup")).toBeTruthy();

    // Duplicate creates "Survival (2)", selects it, and switches the open dialog onto the copy
    fireEvent.click(d.getByRole("button", { name: "Duplicate" }));
    await waitFor(() => expect(api.duplicate).toHaveBeenCalledWith("survival"));
    await waitFor(() =>
      expect(screen.getByLabelText<HTMLInputElement>("Name").value).toBe("Survival (2)"),
    );
    expect(await d.findByRole("heading", { name: "Survival (2)" })).toBeTruthy();

    // Duplicating "Survival (2)" increments to "Survival (3)" rather than "Survival (2) (2)"
    fireEvent.click(d.getByRole("button", { name: "Duplicate" }));
    await waitFor(() => expect(api.duplicate).toHaveBeenCalledWith("survival-2"));
    await waitFor(() =>
      expect(screen.getByLabelText<HTMLInputElement>("Name").value).toBe("Survival (3)"),
    );
  });

  it("pins a modded loader version in Advanced mode, hides the loader picker in Simple mode, and surfaces loader compatibility errors", async () => {
    const modded = (over: Partial<ProfileSummary> = {}) =>
      profile({
        id: "fabric-pack",
        name: "Fabric Pack",
        minecraft: "1.21.4",
        kind: "modded",
        loader: "fabric",
        loaderVersion: "0.16.9",
        ready: true,
        ...over,
      });

    // Simple mode hides the loader version combobox and automatically picks recommended when saving
    const simple = setup(snap([modded()]));
    await flush();
    const simpleDialog = await openProfileSettings(/Profile: Fabric Pack/);
    const sd = within(simpleDialog);

    fireEvent.click(sd.getByRole("button", { name: "Change version…" }));
    await waitFor(() => expect(simple.api.loaderOptions).toHaveBeenCalledWith("1.21.4"));
    await waitFor(() =>
      expect(sd.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "1.21.4",
      ),
    );
    expect(sd.queryByRole("combobox", { name: "Fabric version" })).toBeNull();
    expect(
      sd.getByText(
        "Worlds and options stay in place. Check that any mods in your mods folder match the new version.",
      ),
    ).toBeTruthy();
    fireEvent.click(sd.getByRole("button", { name: "Save version" }));
    await waitFor(() =>
      expect(simple.api.setVersion).toHaveBeenCalledWith("fabric-pack", "1.21.4", {
        mode: "pinned",
        version: "0.16.10",
      }),
    );
    cleanup();

    // Advanced mode lets the user pin a specific loader version and preserves Save disabled when unchanged
    await saveUiMode("advanced");
    const adv = setup(snap([modded({ loaderVersion: "0.16.10" })]));
    await flush();
    const advDialog = await openProfileSettings(/Profile: Fabric Pack/);
    const ad = within(advDialog);

    fireEvent.click(ad.getByRole("button", { name: "Change version…" }));
    await waitFor(() =>
      expect(ad.getByRole("combobox", { name: "Fabric version" }).textContent).toContain("0.16.10"),
    );
    const saveVersion = ad.getByRole<HTMLButtonElement>("button", { name: "Save version" });
    expect(saveVersion.disabled).toBe(true);

    fireEvent.click(ad.getByRole("combobox", { name: "Fabric version" }));
    const loaderOption = await screen.findByRole("option", { name: /0\.16\.9/ });
    fireEvent.pointerDown(loaderOption);
    fireEvent.click(loaderOption);
    expect(saveVersion.disabled).toBe(false);
    fireEvent.click(saveVersion);

    await waitFor(() =>
      expect(adv.api.setVersion).toHaveBeenCalledWith("fabric-pack", "1.21.4", {
        mode: "pinned",
        version: "0.16.9",
      }),
    );
    expect(await ad.findByText("Fabric 1.21.4 · 0.16.9")).toBeTruthy();
    expect(ad.getByText("Needs setup")).toBeTruthy();
    cleanup();

    // Loader compatibility errors: status "none", status "unavailable", and loaderOptions rejection
    for (const scenario of [
      {
        setupMock: (api: ReturnType<typeof setup>["api"]) =>
          api.loaderOptions.mockResolvedValue([
            {
              loader: "fabric",
              status: "none",
              versions: [],
              recommended: null,
              stale: false,
              error: null,
            },
          ]),
        expected: "Fabric has no version for 1.21.4",
      },
      {
        setupMock: (api: ReturnType<typeof setup>["api"]) =>
          api.loaderOptions.mockResolvedValue([
            {
              loader: "fabric",
              status: "unavailable",
              versions: [],
              recommended: null,
              stale: false,
              error: "offline",
            },
          ]),
        expected: "Fabric could not be checked",
      },
      {
        setupMock: (api: ReturnType<typeof setup>["api"]) =>
          api.loaderOptions.mockRejectedValue(new Error("network down")),
        expected: "Could not check which loaders fit. Check your connection.",
      },
    ]) {
      await saveUiMode("advanced");
      const errSetup = setup(snap([modded({ loaderVersion: "0.16.10" })]));
      scenario.setupMock(errSetup.api);
      await flush();

      const errDialog = await openProfileSettings(/Profile: Fabric Pack/);
      const ed = within(errDialog);
      fireEvent.click(ed.getByRole("button", { name: "Change version…" }));
      expect((await ed.findByRole("alert")).textContent).toBe(scenario.expected);
      expect(ed.getByRole<HTMLButtonElement>("button", { name: "Save version" }).disabled).toBe(
        true,
      );
      cleanup();
    }
  });

  it("handles versionCatalog errors with Try again, stale notice, empty filtered list, setVersion rejection, and Cancel", async () => {
    await saveUiMode("advanced");
    const { api } = setup(snap([profile()]));
    api.versionCatalog.mockRejectedValueOnce(new BackendError("offline", "catalog_unavailable"));
    api.versionCatalog.mockResolvedValueOnce({
      entries: [
        { id: "1.21.5", kind: "release", releaseTime: "2025-03-25T10:00:00+00:00" },
        { id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" },
        { id: "25w10a", kind: "snapshot", releaseTime: "2025-03-05T10:00:00+00:00" },
      ],
      latestRelease: "1.21.5",
      latestSnapshot: "25w10a",
      fetchedAt: 1_700_000_000,
      origin: "cache_stale",
      stale: true,
      networkError: "offline",
    });
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    fireEvent.click(d.getByRole("button", { name: "Change version…" }));
    expect(await d.findByText("Versions unavailable")).toBeTruthy();
    expect((await d.findByRole("alert")).textContent).toContain(
      "Could not load the version list. Check your connection and try again.",
    );

    fireEvent.click(d.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(api.versionCatalog).toHaveBeenCalledTimes(2));
    expect(await d.findByText(staleNote(1_700_000_000))).toBeTruthy();

    // Empty filtered list disables Save version and shows empty-list copy
    fireEvent.click(d.getByRole("radio", { name: "Historical" }));
    expect(await d.findByText("No versions in this list.")).toBeTruthy();
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Save version" }).disabled).toBe(true);

    // Switching to Snapshots and failing setVersion keeps the editor open with inline alert
    fireEvent.click(d.getByRole("radio", { name: "Snapshots" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "25w10a",
      ),
    );
    api.setVersion.mockRejectedValueOnce(new BackendError("busy", "already_running"));
    fireEvent.click(d.getByRole("button", { name: "Save version" }));
    expect((await d.findByRole("alert")).textContent).toBe(
      "This profile is starting or running. Close the game first.",
    );
    expect(d.getByRole("button", { name: "Save version" })).toBeTruthy();

    // Cancel exits the version editor without altering readiness
    fireEvent.click(d.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(d.queryByRole("button", { name: "Save version" })).toBeNull());
    expect(d.getByRole("button", { name: "Change version…" })).toBeTruthy();
    expect(d.getByText("Ready")).toBeTruthy();
  });

  it("normalizes leading zeroes in window resolution and locks VersionEditor controls while saving", async () => {
    await saveUiMode("advanced");
    const { api } = setup(snap([profile({ resolution: { width: 1280, height: 720 } })]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    const heightInput = d.getByLabelText<HTMLInputElement>("Window height");
    fireEvent.change(heightInput, { target: { value: "0720" } });
    const applyResBtn = d.getByRole<HTMLButtonElement>("button", { name: "Apply size" });
    expect(applyResBtn.disabled).toBe(true);

    let resolveVersion!: (next: ProfilesSnapshot) => void;
    api.setVersion.mockImplementationOnce(
      () =>
        new Promise<ProfilesSnapshot>((resolve) => {
          resolveVersion = resolve;
        }),
    );
    fireEvent.click(d.getByRole("button", { name: "Change version…" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "1.21.4",
      ),
    );
    fireEvent.click(d.getByRole("radio", { name: "Snapshots" }));
    fireEvent.click(d.getByRole("button", { name: "Save version" }));

    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Releases" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);

    await act(async () => {
      resolveVersion(snap([profile({ minecraft: "25w10a", ready: false })]));
      await Promise.resolve();
    });
    await waitFor(() => expect(d.queryByRole("button", { name: "Save version" })).toBeNull());
  });

  it("steps cleanly on ArrowDown when SelectTrigger receives navigation before popup rAF focus settles", async () => {
    setup(snap([profile({ minecraft: "1.21.5" })]));
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);
    fireEvent.click(d.getByRole("button", { name: "Change version…" }));

    const combo = await d.findByRole("combobox", { name: "Minecraft version" });
    const rafSpy = vi.spyOn(window, "requestAnimationFrame").mockImplementation(() => 0);
    try {
      await act(async () => {
        combo.focus();
        fireEvent.keyDown(combo, { key: "Enter" });
        fireEvent.click(combo, { detail: 0 });
        await Promise.resolve();
      });
      const listbox = screen.getByRole("listbox");
      expect(document.activeElement).toBe(combo);

      fireEvent.keyDown(combo, { key: "ArrowDown" });
      await waitFor(() => {
        const targetOption = within(listbox).getByRole("option", { name: /^1\.21\.4/ });
        expect(targetOption.hasAttribute("data-highlighted")).toBe(true);
        expect(document.activeElement).toBe(targetOption);
      });

      fireEvent.click(document.activeElement as HTMLElement, { detail: 0 });
      await waitFor(() => {
        expect(screen.queryByRole("listbox")).toBeNull();
        expect(combo.textContent).toContain("1.21.4");
      });
    } finally {
      rafSpy.mockRestore();
    }
  });
});
