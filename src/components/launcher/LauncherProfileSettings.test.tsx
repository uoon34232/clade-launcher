import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale } from "@/lib/i18n";
import { BackendError } from "@/lib/ipc";
import type { LaunchApi, RunState } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { resetUiSettings } from "@/lib/settings/uiSettings";
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
    setVersion: vi.fn<ProfilesApi["setVersion"]>((id, minecraft, loaderVersion) => {
      current = {
        ...current,
        profiles: current.profiles.map((p) =>
          p.id === id
            ? {
                ...p,
                minecraft,
                loaderVersion:
                  p.kind === "modded"
                    ? loaderVersion?.mode === "pinned"
                      ? loaderVersion.version
                      : "0.16.10"
                    : null,
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
      const copyName = `${src.name} (2)`;
      const copyId = `${src.id}-2`;
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

const flush = async () => act(async () => void (await Promise.resolve()));

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
  it("shows header details, Ready vs Needs setup, playtime, default memory, Vanilla vs Modded notes, and actions", async () => {
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
    expect(d.getByRole("alert").textContent).toBe("The name can't contain control characters.");
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

    // Backend invalid_memory refusal formats {min} and {max} in GB
    api.setMemory.mockRejectedValueOnce(
      new BackendError("unsafe", "invalid_memory", { params: { min: 1024, max: 12288 } }),
    );
    fireEvent.change(slider, { target: { value: "8192" } });
    fireEvent.pointerUp(slider);
    expect((await d.findByRole("alert")).textContent).toBe(
      "That amount of memory isn't safe for this computer. Choose between 1 GB and 12 GB.",
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
    expect((await d.findByRole("alert")).textContent).toBe("Windows couldn't open that folder.");

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

  it("disables edits, verification and removal while launching or playing, keeping Open profile folder and Done active", async () => {
    const { api } = setup(snap([profile({ memoryMb: 6144 })]), {
      running: "survival",
      runState: "launching",
    });
    await flush();

    const dialog = await openProfileSettings();
    const d = within(dialog);

    expect(d.getByLabelText<HTMLInputElement>("Name").disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Launcher default" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Custom" }).disabled).toBe(true);
    expect(d.getByRole<HTMLInputElement>("slider", { name: "Memory for Minecraft" }).disabled).toBe(
      true,
    );
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Verify files" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Remove from Clade…" }).disabled).toBe(
      true,
    );

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
    const oneBanner = screen.getByRole("status");
    expect(oneBanner.textContent).toContain(
      "1 profile folder couldn’t be read (broken-world). Its files were left untouched.",
    );
    fireEvent.click(within(oneBanner).getByRole("button", { name: "Open profiles folder" }));
    await waitFor(() => expect(single.api.openInstancesFolder).toHaveBeenCalledTimes(1));
    cleanup();

    setup(snap([profile()], { unreadable: ["broken-world", "corrupt-pack"] }));
    await flush();
    expect(screen.getByRole("status").textContent).toContain(
      "2 profile folders couldn’t be read (broken-world, corrupt-pack). Their files were left untouched.",
    );
  });
});

describe("Profile settings and unreadable notice in Russian", () => {
  it("renders ProfileDialog, errors and the unreadable folders strip in Russian", async () => {
    setLocale("ru");
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
        d.getAllByRole("alert").some((a) => a.textContent === "Windows не смог открыть эту папку."),
      ).toBe(true),
    );
  });

  it("changes a profile's Minecraft version in place and duplicates a profile onto the new copy", async () => {
    setLocale("en");
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
    expect(await screen.findByLabelText<HTMLInputElement>("Name")).toHaveProperty(
      "value",
      "Survival (2)",
    );
  });
});
