import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  accountsApi,
  accountsEvents,
  NO_ACCOUNTS,
  type AccountsApi,
  type AccountsSnapshot,
} from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import {
  launchApi,
  LaunchRefused,
  NO_EMBED,
  type GameEmbedStatus,
  type LaunchApi,
  type RunState,
} from "@/lib/launch/api";
import { profilesApi, type ProfilesApi, type ProfilesSnapshot } from "@/lib/profiles/api";
import { completeOnboarding, inTauri, type UiSettings } from "@/lib/settings/uiSettings";
import { playSound } from "@/lib/sound/soundService";
import { AnimatedBackground } from "./AnimatedBackground";
import { Hero } from "./Hero";
import type { Problem } from "./LaunchProblemDialog";
import { LauncherDock } from "./LauncherDock";
import type { OnboardingExit } from "./OnboardingDialog";
import type { PlayState } from "./PlayButton";
import { WindowChrome } from "./WindowChrome";

const AccountDialog = lazy(() =>
  import("./AccountDialog").then((m) => ({ default: m.AccountDialog })),
);
const CreateProfileDialog = lazy(() =>
  import("./CreateProfileDialog").then((m) => ({ default: m.CreateProfileDialog })),
);
const LaunchProblemDialog = lazy(() =>
  import("./LaunchProblemDialog").then((m) => ({ default: m.LaunchProblemDialog })),
);
const OnboardingDialog = lazy(() =>
  import("./OnboardingDialog").then((m) => ({ default: m.OnboardingDialog })),
);
const PrepareDialog = lazy(() =>
  import("./PrepareDialog").then((m) => ({ default: m.PrepareDialog })),
);
const ProfileDialog = lazy(() =>
  import("./ProfileDialog").then((m) => ({ default: m.ProfileDialog })),
);
const RemoveProfileDialog = lazy(() =>
  import("./RemoveProfileDialog").then((m) => ({ default: m.RemoveProfileDialog })),
);
const SettingsDialog = lazy(() =>
  import("./SettingsDialog").then((m) => ({ default: m.SettingsDialog })),
);
const SignInDialog = lazy(() =>
  import("./SignInDialog").then((m) => ({ default: m.SignInDialog })),
);

interface Props {
  /** The launcher's saved UI settings, loaded once at startup. */
  initialUi?: UiSettings;
  /** Injectable for tests; the real backend by default. */
  api?: ProfilesApi;
  accountsClient?: AccountsApi;
  launchClient?: LaunchApi;
}

/** Which secondary surface is open. Only one at a time; all of them are `Modal`s. */
type Surface =
  | { kind: "none" }
  | { kind: "create-profile" }
  | { kind: "profile"; id: string }
  | { kind: "settings" }
  | { kind: "prepare"; id: string; verify?: boolean }
  | { kind: "remove-profile"; id: string }
  | { kind: "sign-in"; accountId: string | null }
  | { kind: "account"; id: string };

const EMPTY: ProfilesSnapshot = { profiles: [], selected: null, totalPlaytimeSeconds: 0 };

export function LauncherHome({
  initialUi,
  api = profilesApi,
  accountsClient = accountsApi,
  launchClient = launchApi,
}: Props) {
  const t = useT();
  const [snapshot, setSnapshot] = useState<ProfilesSnapshot>(EMPTY);
  const [instancesError, setInstancesError] = useState<string | null>(null);
  const [accounts, setAccounts] = useState<AccountsSnapshot>(NO_ACCOUNTS);
  // Until the first answer the dock must not claim "sign in" for accounts that simply have not loaded.
  const [accountsLoaded, setAccountsLoaded] = useState(false);
  const [surface, setSurface] = useState<Surface>({ kind: "none" });
  const [runs, setRuns] = useState<Record<string, RunState>>({});
  const [embed, setEmbed] = useState<GameEmbedStatus>(NO_EMBED);
  const stageRef = useRef<HTMLDivElement>(null);
  const [problem, setProblem] = useState<Problem | null>(null);
  // The first-run introduction: only when the backend says it was never finished or skipped.
  const [introduction, setIntroduction] = useState(initialUi?.onboardingDone === false);
  // Secondary modals are code-split and mounted only after their first opening, then kept mounted
  // so exit animations and focus restoration back to the dock trigger stay intact.
  const [mounted, setMounted] = useState<Record<string, boolean | undefined>>(() =>
    initialUi?.onboardingDone === false ? { onboarding: true } : {},
  );
  if (introduction && !mounted.onboarding) {
    setMounted((m) => ({ ...m, onboarding: true }));
  }
  if (surface.kind !== "none" && !mounted[surface.kind]) {
    setMounted((m) => ({ ...m, [surface.kind]: true }));
  }
  if (problem !== null && !mounted.problem) {
    setMounted((m) => ({ ...m, problem: true }));
  }

  const refresh = useCallback(() => {
    api.list().then(setSnapshot, () => setSnapshot(EMPTY)); // no backend, no profiles: nothing is invented
  }, [api]);
  useEffect(refresh, [refresh]);

  const refreshAccounts = useCallback(() => {
    accountsClient
      .list()
      .then(setAccounts, () => setAccounts(NO_ACCOUNTS)) // no backend, no accounts: nothing is invented
      .finally(() => setAccountsLoaded(true));
  }, [accountsClient]);
  useEffect(refreshAccounts, [refreshAccounts]);

  // Avatars are fetched in the background after the list was answered; the backend says when to look again.
  useEffect(() => {
    let stale = false;
    let off: (() => void) | null = null;
    const subscribe = accountsClient.onChanged ?? accountsEvents.onChanged;
    void subscribe(refreshAccounts).then((u) => {
      if (stale) u();
      else off = u;
    });
    return () => {
      stale = true;
      off?.();
    };
  }, [accountsClient, refreshAccounts]);

  // What the backend says about running games: the starting state, "playing", and how each ended.
  useEffect(() => {
    let stale = false;
    let off: (() => void) | null = null;
    launchClient.status().then(
      (list) => !stale && setRuns(Object.fromEntries(list.map((r) => [r.instanceId, r.state]))),
      () => undefined, // no backend, nothing is running
    );
    void launchClient
      .onState((ev) => {
        setRuns((prev) => {
          const next = { ...prev };
          if (ev.state === "stopped") delete next[ev.instanceId];
          else next[ev.instanceId] = ev.state;
          return next;
        });
        if (ev.state === "stopped") {
          setEmbed((cur) => (cur.instanceId === ev.instanceId ? NO_EMBED : cur));
          refresh(); // the playtime moved
          refreshAccounts(); // a launch may have refreshed the sign-in and noted a new skin
          if (ev.exit && ev.exit.kind !== "normal") {
            setProblem({ kind: "ended", profileId: ev.instanceId, exit: ev.exit });
          }
        }
      })
      .then((u) => {
        if (stale) u();
        else off = u;
      });
    return () => {
      stale = true;
      off?.();
    };
  }, [launchClient, refresh, refreshAccounts]);

  // Framed game mode ("Inside Clade"): sync active attachment state and report responsive stage
  // geometry in physical pixels so Rust can size and round the embedded game window to match.
  useEffect(() => {
    if (!launchClient.embedStatus && !launchClient.onEmbedState) return;
    let stale = false;
    let off: (() => void) | null = null;
    const apply = (next: GameEmbedStatus) => {
      if (stale) return;
      setEmbed(next);
      if (next.active) {
        setSurface((cur) => (cur.kind === "none" ? cur : { kind: "none" }));
      }
    };
    launchClient.embedStatus?.().then(apply, () => undefined);
    if (launchClient.onEmbedState) {
      void launchClient.onEmbedState(apply).then((u) => {
        if (stale) u();
        else off = u;
      });
    }
    return () => {
      stale = true;
      off?.();
    };
  }, [launchClient]);

  useEffect(() => {
    const el = stageRef.current;
    const sendBounds = launchClient.setStageBounds;
    if (!el || !inTauri() || !sendBounds) return;
    let last = "";
    const measure = () => {
      const dpr = window.devicePixelRatio || 1;
      const rect = el.getBoundingClientRect();
      const width = Math.max(0, Math.round(rect.width * dpr));
      const height = Math.max(0, Math.round(rect.height * dpr));
      if (width < 64 || height < 64) return;
      const radiusCss =
        Number.parseFloat(window.getComputedStyle(el).borderTopLeftRadius || "16") || 16;
      const next = {
        x: Math.round(rect.left * dpr),
        y: Math.round(rect.top * dpr),
        width,
        height,
        radius: Math.max(0, Math.round(radiusCss * dpr)),
      };
      const key = `${next.x},${next.y},${next.width},${next.height},${next.radius}`;
      if (key === last) return;
      last = key;
      void sendBounds(next).catch(() => undefined);
    };
    measure();
    const ro = typeof ResizeObserver !== "undefined" ? new ResizeObserver(measure) : null;
    ro?.observe(el);
    window.addEventListener("resize", measure);
    return () => {
      ro?.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [launchClient]);

  const toggleMaximize = useCallback(() => {
    if (!launchClient.toggleMaximize) return;
    void launchClient.toggleMaximize().then(setEmbed, () => undefined);
  }, [launchClient]);

  // Choosing an account changes who plays, never what is played: no profile is touched.
  const selectAccount = (id: string) => {
    setAccounts((a) => ({ ...a, active: id }));
    accountsClient.select(id).then(setAccounts, refreshAccounts);
  };

  // "+ Add account" and "Sign in again" both open the sign-in modal; the flow lives there.
  const addAccount = () => setSurface({ kind: "sign-in", accountId: null });

  const select = (id: string) => {
    setSnapshot((s) => ({ ...s, selected: id })); // immediate; the backend remembers it in data/settings.json
    api.select(id).catch(refresh);
  };

  const close = () => setSurface({ kind: "none" });

  // However the introduction ends, it is recorded as seen (permanently); the last page can lead
  // straight into creating a profile or adding an account, which are the normal dialogs.
  const endIntroduction = (how: OnboardingExit) => {
    setIntroduction(false);
    void completeOnboarding();
    // let the introduction finish closing before the next dialog takes focus
    if (how === "create-profile")
      window.setTimeout(() => setSurface({ kind: "create-profile" }), 0);
    if (how === "add-account") window.setTimeout(addAccount, 0);
  };
  const selectedProfile = snapshot.profiles.find((p) => p.id === snapshot.selected) ?? null;

  // Starts the game. The backend does every check; a refusal comes back as a reason, never a guess.
  const launch = (id: string) => {
    setRuns((r) => ({ ...r, [id]: "launching" }));
    launchClient.launch(id).catch((e: unknown) => {
      setRuns((r) => {
        const next = { ...r };
        delete next[id];
        return next;
      });
      if (e instanceof LaunchRefused && e.code === "needs_repair") {
        setSurface({ kind: "prepare", id }); // repair through the same preparation
      } else if (e instanceof LaunchRefused && e.code === "already_running") {
        setRuns((r) => ({ ...r, [id]: "playing" }));
      } else {
        setProblem({
          kind: "refused",
          profileId: id,
          code: e instanceof LaunchRefused ? e.code : "unknown",
          accountId: e instanceof LaunchRefused ? e.accountId : null,
        });
      }
    });
  };

  // Game files (set up, verify, repair) and launching need the ACTIVE account to be able to play:
  // one that is missing or must sign in again blocks them (the backend refuses them too). Creating
  // and choosing profiles never needs an account. Another, ready account never stands in.
  const activeAccount = accounts.accounts.find((a) => a.id === accounts.active) ?? null;
  const accountReady = !accountsLoaded || (activeAccount !== null && !activeAccount.needsSignIn);
  // For an account that must sign in again, the same account signs in (no duplicate is created).
  const signInForGame = () =>
    setSurface({
      kind: "sign-in",
      accountId: activeAccount?.needsSignIn ? activeAccount.id : null,
    });

  // The primary action: set the profile up, or play it; with none, start creating one; without an
  // account that can play, sign in first.
  const play = () => {
    if (!selectedProfile) setSurface({ kind: "create-profile" });
    else if (!accountReady) signInForGame();
    else if (!selectedProfile.ready) setSurface({ kind: "prepare", id: selectedProfile.id });
    else launch(selectedProfile.id);
  };

  const prepareSurface = surface.kind === "prepare" ? surface : null;
  const preparing = prepareSurface
    ? (snapshot.profiles.find((p) => p.id === prepareSurface.id) ?? null)
    : null;
  const editingProfile =
    surface.kind === "profile"
      ? (snapshot.profiles.find((p) => p.id === surface.id) ?? null)
      : null;
  const editingRun = editingProfile ? runs[editingProfile.id] : undefined;
  const removing =
    surface.kind === "remove-profile"
      ? (snapshot.profiles.find((p) => p.id === surface.id) ?? null)
      : null;
  const unreadable = snapshot.unreadable ?? [];
  if (unreadable.length === 0 && instancesError !== null) setInstancesError(null);
  const run = selectedProfile ? runs[selectedProfile.id] : undefined;
  const playState: PlayState =
    run ??
    (selectedProfile && !accountReady
      ? "signin"
      : preparing && selectedProfile && preparing.id === selectedProfile.id && preparing.ready
        ? "repairing"
        : selectedProfile && !selectedProfile.ready
          ? "setup"
          : "play");

  const openCreate = () => {
    playSound("click");
    setSurface({ kind: "create-profile" });
  };
  const signingIn =
    surface.kind === "sign-in" && surface.accountId
      ? (accounts.accounts.find((a) => a.id === surface.accountId) ?? null)
      : null;
  const managed =
    surface.kind === "account"
      ? (accounts.accounts.find((a) => a.id === surface.id) ?? null)
      : null;
  const bgPaused =
    embed.active ||
    surface.kind !== "none" ||
    introduction ||
    problem !== null ||
    Object.keys(runs).length > 0;

  return (
    <TooltipProvider delay={350}>
      <main className="relative h-full w-full overflow-hidden">
        <AnimatedBackground paused={bgPaused} />
        <WindowChrome
          framed={embed.active}
          maximized={embed.maximized}
          onToggleMaximize={toggleMaximize}
        />
        <div
          ref={stageRef}
          data-testid="game-stage"
          aria-hidden={!embed.active}
          className={
            embed.active
              ? "bg-graphite/95 ring-graphite/15 pointer-events-none absolute top-11 right-3.5 bottom-3.5 left-3.5 rounded-2xl shadow-[0_24px_60px_-20px_rgb(43_38_34/0.55)] ring-1"
              : "pointer-events-none invisible absolute top-11 right-3.5 bottom-3.5 left-3.5 rounded-2xl"
          }
        />
        {!embed.active && (
          <section className="absolute inset-0 grid place-items-center pb-[clamp(6rem,19vh,12rem)]">
            <Hero
              lastPhraseId={initialUi?.lastPhrase}
              playtimeSeconds={snapshot.totalPlaytimeSeconds}
            />
          </section>
        )}
        {!embed.active && unreadable.length > 0 && (
          <div
            role="status"
            className="dock-rise pointer-events-none absolute inset-x-0 bottom-[calc(clamp(1.5rem,6vh,3.5rem)+4.625rem)] flex justify-center px-6"
          >
            <div className="bg-sand/75 text-graphite ring-graphite/10 pointer-events-auto flex max-w-[min(42rem,100%)] flex-wrap items-center justify-center gap-x-3 gap-y-1 rounded-2xl px-3.5 py-2 text-center text-[0.8125rem] leading-snug ring-1">
              <span className="text-ink-muted">
                {t("profile.unreadableNotice", {
                  count: unreadable.length,
                  folders: unreadable.join(", "),
                })}
              </span>
              <button
                type="button"
                onClick={() => {
                  playSound("click");
                  setInstancesError(null);
                  void api
                    .openInstancesFolder()
                    .catch((e: unknown) => setInstancesError(describeError("profile", e)));
                }}
                className="text-graphite decoration-graphite/35 hover:text-clay-deep hover:decoration-clay-deep focus-visible:ring-clay/45 cursor-default rounded-md font-medium underline underline-offset-3 transition-colors outline-none focus-visible:ring-2"
              >
                {t("profile.openInstancesFolder")}
              </button>
              {instancesError && (
                <span role="alert" className="text-clay-deep">
                  {instancesError}
                </span>
              )}
            </div>
          </div>
        )}
        {!embed.active && (
          <LauncherDock
            accounts={accounts.accounts}
            activeAccountId={accounts.active}
            onSelectAccount={selectAccount}
            onManageAccount={(id) => setSurface({ kind: "account", id })}
            onAddAccount={addAccount}
            profiles={snapshot.profiles}
            selectedProfileId={snapshot.selected}
            onSelectProfile={select}
            onCreateProfile={openCreate}
            onOpenProfileSettings={() =>
              selectedProfile && setSurface({ kind: "profile", id: selectedProfile.id })
            }
            onOpenProfileFolder={() =>
              selectedProfile && void api.openFolder(selectedProfile.id).catch(() => undefined)
            }
            onVerifyProfile={() =>
              selectedProfile &&
              setSurface({ kind: "prepare", id: selectedProfile.id, verify: true })
            }
            verifyDisabled={run !== undefined || !accountReady}
            removeDisabled={run !== undefined}
            onRemoveProfile={() =>
              selectedProfile && setSurface({ kind: "remove-profile", id: selectedProfile.id })
            }
            onPlay={play}
            playState={playState}
            onOpenSettings={() => setSurface({ kind: "settings" })}
          />
        )}
        <Suspense fallback={null}>
          {mounted.onboarding && <OnboardingDialog open={introduction} onExit={endIntroduction} />}
          {mounted["create-profile"] && (
            <CreateProfileDialog
              open={surface.kind === "create-profile"}
              onOpenChange={(o) => !o && close()}
              existingNames={snapshot.profiles.map((p) => p.name)}
              onCreated={setSnapshot}
              api={api}
            />
          )}
          {mounted.profile && (
            <ProfileDialog
              profile={editingProfile}
              profiles={snapshot.profiles}
              running={editingRun !== undefined}
              verifyDisabled={editingRun !== undefined || !accountReady}
              removeDisabled={editingRun !== undefined}
              onClose={close}
              onUpdated={setSnapshot}
              onDuplicated={(next) => {
                setSnapshot(next);
                if (next.selected) setSurface({ kind: "profile", id: next.selected });
              }}
              onVerify={() =>
                editingProfile &&
                setSurface({ kind: "prepare", id: editingProfile.id, verify: true })
              }
              onRemove={() =>
                editingProfile && setSurface({ kind: "remove-profile", id: editingProfile.id })
              }
              api={api}
            />
          )}
          {mounted.prepare && (
            <PrepareDialog
              profile={preparing}
              verify={prepareSurface?.verify === true}
              onClose={close}
              onPrepared={setSnapshot}
              api={api}
            />
          )}
          {mounted["remove-profile"] && (
            <RemoveProfileDialog
              profile={removing}
              onClose={close}
              onRemoved={(next) => {
                setSnapshot(next); // the backend's list and a valid selection; no restart needed
                close();
              }}
              api={api}
            />
          )}
          {mounted.problem && (
            <LaunchProblemDialog
              problem={problem}
              onClose={() => setProblem(null)}
              onRepair={(id) => {
                setProblem(null);
                setSurface({ kind: "prepare", id });
              }}
              onSignIn={(accountId) => {
                setProblem(null);
                setSurface({ kind: "sign-in", accountId });
              }}
              onRetry={(id) => {
                setProblem(null);
                launch(id);
              }}
              onOpenLogs={(id) => void launchClient.openLogs(id).catch(() => undefined)}
              onVaultReset={(snapshot) => {
                setAccounts(snapshot);
                // "Start fresh" archived the old vault: the account must sign in again before it can play.
                setProblem((cur) =>
                  cur?.kind === "refused"
                    ? { ...cur, code: "needs_sign_in", accountId: snapshot.active }
                    : cur,
                );
              }}
              accountsClient={accountsClient}
            />
          )}
          {mounted.settings && (
            <SettingsDialog
              open={surface.kind === "settings"}
              onOpenChange={(o) => !o && close()}
            />
          )}
          {mounted["sign-in"] && (
            <SignInDialog
              open={surface.kind === "sign-in"}
              onOpenChange={(o) => !o && close()}
              account={signingIn ? { id: signingIn.id, username: signingIn.username } : null}
              onSignedIn={(snapshot) => {
                setAccounts(snapshot);
                close();
              }}
              onVaultReset={setAccounts}
              api={accountsClient}
            />
          )}
          {mounted.account && (
            <AccountDialog
              account={managed}
              capabilities={accounts.capabilities}
              onClose={close}
              onSignInAgain={(id) => setSurface({ kind: "sign-in", accountId: id })}
              onRemoved={(snapshot) => {
                setAccounts(snapshot);
                close();
              }}
              onVaultReset={setAccounts}
              api={accountsClient}
            />
          )}
        </Suspense>
      </main>
    </TooltipProvider>
  );
}
