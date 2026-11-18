import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BASE_PHRASES, phraseText } from "@/features/launcher/phrases";
import type {
  LoaderOption,
  ProfilesApi,
  ProfilesSnapshot,
  VersionCatalog,
} from "@/lib/profiles/api";
import { BackendError } from "@/lib/ipc";
import { LauncherHome } from "./LauncherHome";
import { readyAccountsClient } from "@/test/testAccounts";

vi.mock("@/lib/sound/soundService", () => ({ playSound: vi.fn(() => true) }));

const CATALOG: VersionCatalog = {
  entries: [
    { id: "26.4-snapshot-2", kind: "snapshot", releaseTime: "2026-09-29T11:32:57+00:00" },
    { id: "26.3", kind: "release", releaseTime: "2026-09-01T10:00:00+00:00" },
    { id: "1.21.4", kind: "release", releaseTime: "2024-12-03T10:12:57+00:00" },
    { id: "b1.7.3", kind: "old_beta", releaseTime: "2011-07-08T00:00:00+00:00" },
  ],
  latestRelease: "26.3",
  latestSnapshot: "26.4-snapshot-2",
  fetchedAt: 1_700_000_000,
  origin: "network",
  stale: false,
  networkError: null,
};

const OPTIONS: LoaderOption[] = [
  {
    loader: "fabric",
    status: "ok",
    versions: [
      { version: "0.19.5", channel: "stable" },
      { version: "0.18.0", channel: "stable" },
    ],
    recommended: "0.19.5",
    stale: false,
    error: null,
  },
  {
    loader: "forge",
    status: "ok",
    versions: [{ version: "54.1.14", channel: "recommended" }],
    recommended: "54.1.14",
    stale: false,
    error: null,
  },
  {
    loader: "neoforge",
    status: "none",
    versions: [],
    recommended: null,
    stale: false,
    error: null,
  },
  {
    loader: "quilt",
    status: "unavailable",
    versions: [],
    recommended: null,
    stale: false,
    error: "the loader's site could not be reached",
  },
];

const profile = (
  over: Partial<ProfilesSnapshot["profiles"][number]> = {},
): ProfilesSnapshot["profiles"][number] => ({
  id: "survival",
  name: "Survival",
  minecraft: "1.21.4",
  kind: "vanilla",
  loader: null,
  loaderVersion: null,
  ready: false,
  playtimeSeconds: 0,
  ...over,
});

function makeApi(initial: ProfilesSnapshot) {
  let snapshot = initial;
  const list = vi.fn<ProfilesApi["list"]>(() => Promise.resolve(snapshot));
  const select = vi.fn<ProfilesApi["select"]>((id) => {
    snapshot = { ...snapshot, selected: id };
    return Promise.resolve();
  });
  const versionCatalog = vi.fn<ProfilesApi["versionCatalog"]>(() => Promise.resolve(CATALOG));
  const loaderOptions = vi.fn<ProfilesApi["loaderOptions"]>(() => Promise.resolve(OPTIONS));
  const create = vi.fn<ProfilesApi["create"]>((req) => {
    const modded = req.profile.type === "modded" ? req.profile : null;
    const p = profile({
      id: req.name.toLowerCase(),
      name: req.name,
      minecraft: req.minecraft,
      kind: req.profile.type,
      loader: modded ? modded.loader : null,
      loaderVersion:
        modded && modded.loader_version.mode === "pinned" ? modded.loader_version.version : null,
    });
    snapshot = { ...snapshot, profiles: [...snapshot.profiles, p], selected: p.id };
    return Promise.resolve(snapshot);
  });
  const prepare = vi.fn<ProfilesApi["prepare"]>(() => Promise.resolve(snapshot));
  const cancelPrepare = vi.fn<ProfilesApi["cancelPrepare"]>(() => Promise.resolve());
  const onPrepareProgress = vi.fn<ProfilesApi["onPrepareProgress"]>(() =>
    Promise.resolve(() => undefined),
  );
  return {
    list,
    select,
    versionCatalog,
    loaderOptions,
    create,
    rename: vi.fn<ProfilesApi["rename"]>(() => Promise.reject(new Error("not used in this test"))),
    setMemory: vi.fn<ProfilesApi["setMemory"]>(() =>
      Promise.reject(new Error("not used in this test")),
    ),
    openFolder: vi.fn<ProfilesApi["openFolder"]>(() => Promise.resolve()),
    openInstancesFolder: vi.fn<ProfilesApi["openInstancesFolder"]>(() => Promise.resolve()),
    prepare,
    cancelPrepare,
    remove: vi.fn<ProfilesApi["remove"]>(() => Promise.reject(new Error("not used in this test"))),
    onPrepareProgress,
  };
}

const empty: ProfilesSnapshot = { profiles: [], selected: null, totalPlaytimeSeconds: 0 };
const flush = async () => act(async () => void (await Promise.resolve()));
const dialog = () => screen.getByRole("dialog");

beforeEach(() => {
  // jsdom lacks these; the dialog and select primitives expect them.
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

describe("LauncherHome", () => {
  it("shows the headline and the dock controls", async () => {
    render(
      <LauncherHome
        api={makeApi({ profiles: [profile()], selected: "survival", totalPlaytimeSeconds: 0 })}
      />,
    );
    await flush();
    expect(BASE_PHRASES.map((p) => phraseText(p))).toContain(
      screen.getByRole("heading", { level: 1 }).textContent,
    );
    // No account yet: the primary action asks to sign in instead of offering a download that would be refused.
    expect(screen.getByRole("button", { name: "Sign in to continue" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Set up" })).toBeNull();
    expect(screen.getByRole("button", { name: "Settings" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add an account" })).toBeTruthy();
  });

  it("shows the selected profile concisely in the dock", async () => {
    const api = makeApi({
      profiles: [
        profile(),
        profile({
          id: "modded",
          name: "Modded",
          minecraft: "1.20.1",
          kind: "modded",
          loader: "fabric",
          loaderVersion: "0.16.9",
        }),
      ],
      selected: "modded",
      totalPlaytimeSeconds: 0,
    });
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();
    const trigger = screen.getByRole("button", { name: "Profile: Modded. Change profile" });
    // The collapsed dock button shows only the profile's name: no version, loader or "Vanilla".
    expect(trigger.textContent).toBe("Modded");
    expect(trigger.textContent).not.toMatch(/Fabric|1\.20\.1|Vanilla|·/);
  });

  it("shows no fake profiles when there is no backend or no profile", async () => {
    const api = makeApi(empty);
    api.list.mockRejectedValueOnce(new Error("no backend"));
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();
    expect(screen.getByRole("button", { name: /Create a profile/ })).toBeTruthy();
    expect(screen.queryByText(/Vanilla 1\./)).toBeNull();
  });
});

describe("total playtime under the subtitle", () => {
  it("shows the real backend total, understated", async () => {
    render(
      <LauncherHome
        api={makeApi({
          profiles: [profile()],
          selected: "survival",
          totalPlaytimeSeconds: 27 * 3600 + 42 * 60,
        })}
      />,
    );
    await flush();
    expect(screen.getByText("Played 27h 42m")).toBeTruthy();
  });

  it("is hidden gracefully for a brand-new launcher", async () => {
    render(<LauncherHome api={makeApi(empty)} accountsClient={readyAccountsClient()} />);
    await flush();
    expect(screen.queryByText(/Played/)).toBeNull();
  });
});

describe("Create profile", () => {
  async function openDialog(api: ReturnType<typeof makeApi>, snapshot?: ProfilesSnapshot) {
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();
    void snapshot;
    fireEvent.click(screen.getByRole("button", { name: /Create a profile/ }));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
    await flush();
  }

  it("asks for a name, a Minecraft version and a type, and never mentions loaders for Vanilla", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    const d = within(dialog());
    expect(d.getByText("New profile")).toBeTruthy();
    expect(d.getByLabelText("Name")).toBeTruthy();
    expect(d.getByRole("radio", { name: "Vanilla" }).getAttribute("aria-checked")).toBe("true");
    expect(d.getByRole("radio", { name: "Modded" }).getAttribute("aria-checked")).toBe("false");
    // The loader fields exist but are hidden and inert while Vanilla is selected, and nothing was fetched for them.
    const reveal = dialog().querySelector(".reveal") as HTMLElement;
    expect(reveal.dataset.open).toBe("false");
    expect(reveal.hasAttribute("inert")).toBe(true);
    expect(api.loaderOptions).not.toHaveBeenCalled();
    // The newest official release is preselected.
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain("26.3"),
    );
  });

  it("validates the name and refuses duplicates", async () => {
    const api = makeApi({ profiles: [profile()], selected: "survival", totalPlaytimeSeconds: 0 });
    render(<LauncherHome api={api} accountsClient={readyAccountsClient()} />);
    await flush();
    fireEvent.click(screen.getByRole("button", { name: /Profile: Survival/ }));
    fireEvent.click(await screen.findByRole("menuitem", { name: /Create profile/ }));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
    await flush();
    const d = within(dialog());
    const create = d.getByRole<HTMLButtonElement>("button", { name: "Create profile" });
    expect(create.disabled).toBe(true);
    const input = d.getByLabelText("Name");
    fireEvent.change(input, { target: { value: "  survival " } });
    fireEvent.blur(input);
    expect(d.getByRole("alert").textContent).toMatch(/already have a profile/);
    expect(create.disabled).toBe(true);
    fireEvent.change(input, { target: { value: "Creative" } });
    await waitFor(() => expect(create.disabled).toBe(false));
  });

  it("creates a Vanilla profile through the backend and selects it in the dock", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    const d = within(dialog());
    fireEvent.change(d.getByLabelText("Name"), { target: { value: "Survival" } });
    await waitFor(() =>
      expect(d.getByRole<HTMLButtonElement>("button", { name: "Create profile" }).disabled).toBe(
        false,
      ),
    );
    fireEvent.click(d.getByRole("button", { name: "Create profile" }));
    await waitFor(() => expect(api.create).toHaveBeenCalledTimes(1));
    expect(api.create.mock.calls[0][0]).toEqual({
      name: "Survival",
      minecraft: "26.3",
      profile: { type: "vanilla" },
    });
    expect(api.loaderOptions).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(
      await screen.findByRole("button", { name: "Profile: Survival. Change profile" }),
    ).toBeTruthy();
  });

  it("Modded reveals the loader fields, offers only compatible loaders, and creates with the recommended version", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    const d = within(dialog());
    fireEvent.change(d.getByLabelText("Name"), { target: { value: "Modded" } });
    fireEvent.click(d.getByRole("radio", { name: "Modded" }));

    const reveal = dialog().querySelector(".reveal") as HTMLElement;
    expect(reveal.dataset.open).toBe("true");
    expect(reveal.hasAttribute("inert")).toBe(false);
    await waitFor(() => expect(api.loaderOptions).toHaveBeenCalledWith("26.3"));

    // Fabric and Forge are real options; NeoForge has no version and Quilt could not be checked: disabled, with a reason.
    await waitFor(() =>
      expect(d.getByRole<HTMLButtonElement>("radio", { name: "Fabric" }).disabled).toBe(false),
    );
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Forge" }).disabled).toBe(false);
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "NeoForge" }).disabled).toBe(true);
    expect(d.getByRole<HTMLButtonElement>("radio", { name: "Quilt" }).disabled).toBe(true);
    expect(dialog().textContent).toMatch(/NeoForge has no version for 26\.3/);
    expect(dialog().textContent).toMatch(/Quilt could not be checked/);
    // The first compatible loader is chosen, with its recommended version.
    expect(d.getByRole("radio", { name: "Fabric" }).getAttribute("aria-checked")).toBe("true");
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Loader version" }).textContent).toContain("0.19.5"),
    );

    fireEvent.click(d.getByRole("radio", { name: "Forge" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Loader version" }).textContent).toContain("54.1.14"),
    );

    fireEvent.click(d.getByRole("button", { name: "Create profile" }));
    await waitFor(() => expect(api.create).toHaveBeenCalled());
    expect(api.create.mock.calls[0][0]).toEqual({
      name: "Modded",
      minecraft: "26.3",
      profile: {
        type: "modded",
        loader: "forge",
        loader_version: { mode: "pinned", version: "54.1.14" },
      },
    });
    expect(
      await screen.findByRole("button", { name: "Profile: Modded. Change profile" }),
    ).toBeTruthy();
  });

  it("filters the version list: releases by default, snapshots and historical on request", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    const d = within(dialog());
    expect(d.getByRole("radio", { name: "Releases" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(d.getByRole("radio", { name: "Snapshots" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "26.4-snapshot-2",
      ),
    );
    fireEvent.click(d.getByRole("radio", { name: "Historical" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain(
        "b1.7.3",
      ),
    );
    fireEvent.click(d.getByRole("radio", { name: "Releases" }));
    await waitFor(() =>
      expect(d.getByRole("combobox", { name: "Minecraft version" }).textContent).toContain("26.3"),
    );
  });

  it("segmented controls work with the arrow keys", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    const d = within(dialog());
    const vanilla = d.getByRole("radio", { name: "Vanilla" });
    vanilla.focus();
    fireEvent.keyDown(vanilla, { key: "ArrowRight" });
    expect(d.getByRole("radio", { name: "Modded" }).getAttribute("aria-checked")).toBe("true");
    expect(document.activeElement).toBe(d.getByRole("radio", { name: "Modded" }));
    fireEvent.keyDown(document.activeElement as Element, { key: "ArrowLeft" });
    expect(d.getByRole("radio", { name: "Vanilla" }).getAttribute("aria-checked")).toBe("true");
  });

  it("explains an offline saved list, and a backend error on creation stays inline", async () => {
    const api = makeApi(empty);
    api.versionCatalog.mockResolvedValueOnce({
      ...CATALOG,
      stale: true,
      origin: "cache_stale",
      networkError: "Mojang could not be reached",
    });
    api.create.mockRejectedValueOnce(
      new BackendError("duplicate", "duplicate_name", { params: { name: "X" } }),
    );
    await openDialog(api);
    const d = within(dialog());
    expect(await d.findByText(/Offline: using the version list from/)).toBeTruthy();
    fireEvent.change(d.getByLabelText("Name"), { target: { value: "X" } });
    await waitFor(() =>
      expect(d.getByRole<HTMLButtonElement>("button", { name: "Create profile" }).disabled).toBe(
        false,
      ),
    );
    fireEvent.click(d.getByRole("button", { name: "Create profile" }));
    expect(await d.findByText(/already exists/)).toBeTruthy();
    expect(screen.getByRole("dialog")).toBeTruthy(); // stays open so the user can fix it
  });

  it("cancel closes without creating anything, and reopening starts from a fresh form", async () => {
    const api = makeApi(empty);
    await openDialog(api);
    fireEvent.change(within(dialog()).getByLabelText("Name"), { target: { value: "Draft" } });
    fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(api.create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /Create a profile/ }));
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
    expect(within(dialog()).getByLabelText<HTMLInputElement>("Name").value).toBe("");
  });
});
