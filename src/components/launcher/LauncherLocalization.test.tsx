// English <-> Russian in the real home screen: switching live, keeping the screen and focus,
// phrase identity, persistence and the first-run default.
import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BASE_PHRASES, phraseText } from "@/features/launcher/phrases";
import type { AccountsApi, AccountsSnapshot } from "@/lib/accounts/api";
import { getLocale, resolveLocale, setLocale } from "@/lib/i18n";
import { chooseLanguage } from "@/lib/i18n/language";
import { BackendError } from "@/lib/ipc";
import type { LaunchApi } from "@/lib/launch/api";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { loadUiSettings, resetUiSettings, saveUiMode } from "@/lib/settings/uiSettings";
import { appearanceStubs, recoveryStubs } from "@/test/testAccounts";

const invoke = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (c: string, a?: Record<string, unknown>) => invoke(c, a),
}));
vi.mock("@/lib/sound/soundService", () => ({
  playSound: vi.fn(() => true),
  sound: {
    getSettings: () => ({ enabled: true, volume: 0.5 }),
    subscribe: () => () => undefined,
    setSettings: () => undefined,
  },
}));

import { LauncherHome } from "./LauncherHome";
import { PlayButton } from "./PlayButton";

const profile = (over: Partial<ProfileSummary> = {}): ProfileSummary => ({
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: true,
  playtimeSeconds: 0,
  ...over,
});

const ACCOUNTS: AccountsSnapshot = {
  accounts: [
    {
      id: "a",
      username: "Alex",
      uuid: "00000000-0000-4000-8000-00000000a001",
      needsSignIn: false,
      skin: null,
    },
    {
      id: "b",
      username: "Blake",
      uuid: "00000000-0000-4000-8000-00000000b002",
      needsSignIn: true,
      skin: null,
    },
  ],
  active: "a",
  capabilities: { signIn: true, skin: false, cape: false },
};

function apis(snapshot: ProfilesSnapshot, accounts: AccountsSnapshot = ACCOUNTS) {
  const profiles = {
    list: vi.fn<ProfilesApi["list"]>(() => Promise.resolve(snapshot)),
    select: vi.fn<ProfilesApi["select"]>(() => Promise.resolve()),
    versionCatalog: vi.fn<ProfilesApi["versionCatalog"]>(() =>
      Promise.resolve({
        entries: [{ id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" }],
        latestRelease: "1.21.4",
        latestSnapshot: null,
        fetchedAt: 1_700_000_000,
        origin: "cache_stale",
        stale: true,
        networkError: "offline",
      }),
    ),
    loaderOptions: vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve([])),
    create: vi.fn<ProfilesApi["create"]>(() =>
      Promise.reject(new BackendError("dup", "duplicate_name", { params: { name: "Survival" } })),
    ),
    rename: vi.fn<ProfilesApi["rename"]>(() => Promise.reject(new Error("not used in this test"))),
    setMemory: vi.fn<ProfilesApi["setMemory"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setResolution: vi.fn<ProfilesApi["setResolution"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setJvmArgs: vi.fn<ProfilesApi["setJvmArgs"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    setVersion: vi.fn<ProfilesApi["setVersion"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    duplicate: vi.fn<ProfilesApi["duplicate"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    openFolder: vi.fn<ProfilesApi["openFolder"]>(() => Promise.resolve()),
    openInstancesFolder: vi.fn<ProfilesApi["openInstancesFolder"]>(() => Promise.resolve()),
    prepare: vi.fn<ProfilesApi["prepare"]>(() => new Promise<ProfilesSnapshot>(() => undefined)),
    cancelPrepare: vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve()),
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress: vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  };
  const acc = {
    list: vi.fn<AccountsApi["list"]>(() => Promise.resolve(accounts)),
    select: vi.fn<AccountsApi["select"]>(() => Promise.resolve(accounts)),
    remove: vi.fn<AccountsApi["remove"]>(() => Promise.resolve(accounts)),
    ...appearanceStubs(),
    ...recoveryStubs(),
    vaultStatus: vi.fn<AccountsApi["vaultStatus"]>(() =>
      Promise.resolve({ exists: false, unlocked: false }),
    ),
    createVault: vi.fn<AccountsApi["createVault"]>(() => Promise.resolve()),
    unlockVault: vi.fn<AccountsApi["unlockVault"]>(() => Promise.resolve()),
    signIn: vi.fn<AccountsApi["signIn"]>(() => new Promise(() => undefined)),
    cancelSignIn: vi.fn<AccountsApi["cancelSignIn"]>(() => Promise.resolve()),
    onSignInProgress: vi.fn<AccountsApi["onSignInProgress"]>(() =>
      Promise.resolve(() => undefined),
    ),
  };
  const launch = {
    launch: vi.fn<LaunchApi["launch"]>(() => Promise.resolve()),
    status: vi.fn<LaunchApi["status"]>(() => Promise.resolve([])),
    openLogs: vi.fn<LaunchApi["openLogs"]>(() => Promise.resolve()),
    onState: vi.fn<LaunchApi["onState"]>(() => Promise.resolve(() => undefined)),
  };
  return { profiles, acc, launch };
}

const flush = async () => act(async () => void (await Promise.resolve()));

beforeEach(async () => {
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

async function renderHome(
  snapshot: ProfilesSnapshot = {
    profiles: [profile()],
    selected: "survival",
    totalPlaytimeSeconds: 27 * 3600 + 42 * 60,
  },
  options: {
    accounts?: AccountsSnapshot;
    setup?: (ctx: ReturnType<typeof apis>) => void;
  } = {},
) {
  const ctx = apis(snapshot, options.accounts);
  options.setup?.(ctx);
  render(
    <StrictMode>
      <LauncherHome api={ctx.profiles} accountsClient={ctx.acc} launchClient={ctx.launch} />
    </StrictMode>,
  );
  await flush();
  return ctx;
}

const dock = () => screen.getByRole("navigation", { name: "Clade" });
const dialog = () => screen.getByRole("dialog");

describe("the home screen in Russian", () => {
  it("shows the whole home screen in Russian, with the playtime formatted for the language", async () => {
    setLocale("ru");
    await renderHome();
    expect(screen.getByText("Выбери профиль и войди в свой мир.")).toBeTruthy();
    expect(screen.getByText("В игре 27 ч 42 мин")).toBeTruthy();
    const d = within(dock());
    expect(d.getByRole("button", { name: "Играть" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Аккаунт: Alex. Сменить аккаунт" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Профиль: Survival. Сменить профиль" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Настройки" })).toBeTruthy();
    expect(BASE_PHRASES.map(phraseText)).toContain(
      screen.getByRole("heading", { level: 1 }).textContent,
    );
    expect(document.documentElement.lang).toBe("ru");
  });

  describe("words the other dock states in Russian", () => {
    it("shows setup for an unprepared profile", async () => {
      setLocale("ru");
      await renderHome({
        profiles: [profile({ ready: false })],
        selected: "survival",
        totalPlaytimeSeconds: 0,
      });
      expect(within(dock()).getByRole("button", { name: "Подготовить" })).toBeTruthy();
    });

    it("shows launching while a launch is in flight", async () => {
      setLocale("ru");
      await renderHome(undefined, {
        setup: (ctx) => {
          ctx.launch.launch.mockImplementation(() => new Promise(() => undefined));
        },
      });
      fireEvent.click(within(dock()).getByRole("button", { name: "Играть" }));
      expect(within(dock()).getByRole("button", { name: "Запуск…" })).toBeTruthy();
    });

    it("shows sign-in when the active account needs to sign in again", async () => {
      setLocale("ru");
      await renderHome(undefined, {
        accounts: { ...ACCOUNTS, active: "b" },
      });
      expect(within(dock()).getByRole("button", { name: "Войти для продолжения" })).toBeTruthy();
    });

    it("shows playing while the selected profile is running", async () => {
      setLocale("ru");
      await renderHome(undefined, {
        setup: (ctx) => {
          ctx.launch.status.mockResolvedValue([{ instanceId: "survival", state: "playing" }]);
        },
      });
      expect(within(dock()).getByRole("button", { name: "В игре" })).toBeTruthy();
    });

    it("shows repairing while a ready profile is being repaired", () => {
      setLocale("ru");
      render(
        <StrictMode>
          <PlayButton state="repairing" />
        </StrictMode>,
      );
      const btn = screen.getByRole<HTMLButtonElement>("button", { name: "Восстановление…" });
      expect(btn.disabled).toBe(true);
      expect(btn.getAttribute("aria-busy")).toBe("true");
    });
  });

  it("opens the account menu, the profile menu and Create profile in Russian", async () => {
    setLocale("ru");
    await saveUiMode("advanced");
    await renderHome();
    fireEvent.click(within(dock()).getByRole("button", { name: /Аккаунт: Alex/ }));
    expect(await screen.findByRole("menuitem", { name: "+ Добавить аккаунт" })).toBeTruthy();
    expect(screen.getByText("Войти снова")).toBeTruthy(); // Blake needs to sign in again
    expect(screen.getByRole("menuitem", { name: "Управление: Blake" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());

    fireEvent.click(within(dock()).getByRole("button", { name: /Профиль: Survival/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: "+ Создать профиль" }));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
    const d = within(dialog());
    expect(await d.findByText("Новый профиль")).toBeTruthy();
    expect(d.getByLabelText("Название")).toBeTruthy();
    expect(d.getByRole("radio", { name: "Релизы" })).toBeTruthy();
    expect(d.getByRole("radio", { name: "Ванильный" })).toBeTruthy();
    expect(d.getByRole("radio", { name: "С модами" })).toBeTruthy();
    expect(d.getByRole("button", { name: "Создать профиль" })).toBeTruthy();
    expect(await d.findByText(/Нет сети: используется список версий от/)).toBeTruthy();
    // validation and backend errors come back in Russian too
    fireEvent.change(d.getByLabelText("Название"), { target: { value: "Survival" } });
    fireEvent.blur(d.getByLabelText("Название"));
    expect(await d.findByText("Профиль с таким названием уже есть.")).toBeTruthy();
    fireEvent.change(d.getByLabelText("Название"), { target: { value: "Другой" } });
    await waitFor(() =>
      expect(d.getByRole<HTMLButtonElement>("button", { name: "Создать профиль" }).disabled).toBe(
        false,
      ),
    );
    fireEvent.click(d.getByRole("button", { name: "Создать профиль" }));
    expect(await d.findByText("Профиль «Survival» уже существует.")).toBeTruthy();
  });
});

describe("switching the language live", () => {
  async function openSettings() {
    const ctx = await renderHome();
    fireEvent.click(within(dock()).getByRole("button", { name: "Settings" }));
    await screen.findByRole("dialog");
    return ctx;
  }

  it("English -> Русский -> English updates the screen without closing the modal or losing focus", async () => {
    await openSettings();
    const radios = () =>
      within(within(dialog()).getByRole("radiogroup", { name: /^(Language|Язык)$/ })).getAllByRole(
        "radio",
      );
    expect(within(dialog()).getByText("Settings")).toBeTruthy();
    const russian = within(dialog()).getByRole("radio", { name: "Русский" });
    russian.focus();
    fireEvent.click(russian);
    await flush();

    expect(getLocale()).toBe("ru");
    // the same modal is still open, now in Russian, and the control still has focus
    expect(screen.getAllByRole("dialog")).toHaveLength(1);
    expect(within(dialog()).getByText("Настройки")).toBeTruthy();
    expect(within(dialog()).getByRole("radiogroup", { name: "Режим" })).toBeTruthy();
    expect(within(dialog()).getByRole("radio", { name: "Простой" })).toBeTruthy();
    expect(within(dialog()).getByRole("radio", { name: "Расширенный" })).toBeTruthy();
    expect(within(dialog()).getByText("Звуковые эффекты")).toBeTruthy();
    expect(within(dialog()).getByText("Громкость")).toBeTruthy();
    expect(within(dialog()).getByRole("radiogroup", { name: "Язык" })).toBeTruthy();
    expect(document.activeElement).toBe(within(dialog()).getByRole("radio", { name: "Русский" }));
    expect(radios().map((r) => r.textContent)).toEqual(["English", "Русский"]); // endonyms never change
    expect(radios().map((r) => r.getAttribute("aria-checked"))).toEqual(["false", "true"]);
    // the screen behind it followed
    expect(screen.getByText("Выбери профиль и войди в свой мир.", { selector: "p" })).toBeTruthy();
    expect(screen.getByText("В игре 27 ч 42 мин")).toBeTruthy();

    const english = within(dialog()).getByRole("radio", { name: "English" });
    english.focus(); // a real click focuses the control it lands on
    fireEvent.click(english);
    await flush();
    expect(getLocale()).toBe("en");
    expect(within(dialog()).getByText("Settings")).toBeTruthy();
    expect(screen.getByText("Played 27h 42m")).toBeTruthy();
    expect(document.activeElement).toBe(within(dialog()).getByRole("radio", { name: "English" }));
    expect(document.documentElement.lang).toBe("en");
  });

  it("keeps the headline's phrase id and only re-words it (no re-pick, nothing saved)", async () => {
    await renderHome();
    const h1 = () => screen.getByRole("heading", { level: 1 }).textContent ?? "";
    const shown = BASE_PHRASES.find((ph) => phraseText(ph) === h1());
    expect(shown).toBeTruthy();
    act(() => setLocale("ru"));
    setLocale("ru");
    await flush();
    expect(h1()).toBe(phraseText(shown as (typeof BASE_PHRASES)[number]));
    expect(h1()).toMatch(/[А-Яа-яЁё]/);
    act(() => setLocale("en"));
    expect(h1()).toBe(phraseText(shown as (typeof BASE_PHRASES)[number]));
    expect(BASE_PHRASES.map((p) => p.id)).toContain(shown?.id);
  });

  it("keeps the keyboard order of Settings identical in both languages", async () => {
    const stops = () =>
      [...dialog().querySelectorAll<HTMLElement>("button, input")]
        .filter((el) => el.tabIndex >= 0)
        .map((el) => `${el.tagName}:${el.getAttribute("role") ?? el.getAttribute("type") ?? ""}`);
    await openSettings();
    const english = stops();
    expect(english.length).toBeGreaterThan(3);
    act(() => setLocale("ru"));
    expect(stops()).toEqual(english);
  });

  it("reverts the Settings radio selection and copy to English when set_language rejects", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockImplementation((cmd) => {
      if (cmd === "set_language") return Promise.reject(new Error("read-only stick"));
      return Promise.resolve(null);
    });
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      await openSettings();
      const radios = () =>
        within(
          within(dialog()).getByRole("radiogroup", { name: /^(Language|Язык)$/ }),
        ).getAllByRole("radio");

      expect(within(dialog()).getByText("Settings")).toBeTruthy();
      expect(radios().map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false"]);

      fireEvent.click(within(dialog()).getByRole("radio", { name: "Русский" }));
      expect(getLocale()).toBe("ru");

      await waitFor(() => {
        expect(invoke).toHaveBeenCalledWith("set_language", { language: "ru" });
        expect(getLocale()).toBe("en");
        expect(within(dialog()).getByText("Settings")).toBeTruthy();
      });
      expect(radios().map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false"]);
      expect(document.documentElement.lang).toBe("en");
      expect(screen.getByText("Played 27h 42m")).toBeTruthy();
    } finally {
      warnSpy.mockRestore();
      invoke.mockReset();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    }
  });
});

describe("persistence and the first-run default", () => {
  it("saves an explicit choice through the settings service and it wins on the next start", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    let savedLanguage: string | null = null;
    invoke.mockImplementation((cmd, args) => {
      if (cmd === "set_language") {
        savedLanguage = (args?.language as string) ?? null;
        return Promise.resolve(null);
      }
      if (cmd === "get_ui_settings") {
        return Promise.resolve({ language: savedLanguage, systemLocale: "en-US" });
      }
      return Promise.resolve(null);
    });
    try {
      await chooseLanguage("ru");
      expect(invoke).toHaveBeenCalledWith("set_language", { language: "ru" });
      expect(getLocale()).toBe("ru");
      // A "restart": the backend returns what was saved; an English Windows no longer matters.
      const restarted = await loadUiSettings();
      expect(invoke).toHaveBeenCalledWith("get_ui_settings", undefined);
      expect(restarted.language).toBe("ru");
      expect(resolveLocale(restarted.language, "en-US")).toBe("ru");
      await chooseLanguage("en");
      expect(invoke).toHaveBeenCalledWith("set_language", { language: "en" });
      expect(resolveLocale((await loadUiSettings()).language, "ru-RU")).toBe("en");
    } finally {
      invoke.mockReset();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    }
  });

  it("undoes the switch if it cannot be saved, so the screen never disagrees with the file", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockRejectedValueOnce(new Error("read-only stick"));
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      await chooseLanguage("ru");
      expect(invoke).toHaveBeenCalledWith("set_language", { language: "ru" });
      expect(getLocale()).toBe("en");
    } finally {
      warnSpy.mockRestore();
      invoke.mockReset();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    }
  });

  it("ignores stale UI updates when an earlier save resolves ok while a newer switch is pending, and rolls back to the confirmed locale on failure", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const pending: Array<{ resolve: () => void; reject: (e: unknown) => void }> = [];
    invoke.mockImplementation(
      () =>
        new Promise<void>((resolve, reject) => {
          pending.push({ resolve, reject });
        }),
    );
    try {
      const p1 = chooseLanguage("ru");
      expect(getLocale()).toBe("ru");
      const p2 = chooseLanguage("en");
      expect(getLocale()).toBe("en");

      // Token 1 resolves ok first while token 2 is still in flight: UI stays on "en".
      pending[0]?.resolve();
      await p1;
      expect(getLocale()).toBe("en");

      // Token 2 then succeeds cleanly.
      pending[1]?.resolve();
      await p2;
      expect(getLocale()).toBe("en");

      // Next race: token 3 ("ru") confirms first, then token 4 ("en") fails -> rolls back to "ru".
      const p3 = chooseLanguage("ru");
      const p4 = chooseLanguage("en");
      pending[2]?.resolve();
      await p3;
      expect(getLocale()).toBe("en");

      pending[3]?.reject(new Error("read-only stick"));
      await p4;
      expect(getLocale()).toBe("ru");
    } finally {
      warnSpy.mockRestore();
      invoke.mockReset();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    }
  });
});
