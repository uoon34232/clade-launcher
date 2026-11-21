import {
  Component,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ErrorInfo,
  type ReactNode,
} from "react";
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

type LazyModalComponent = Parameters<typeof lazy>[0] extends () => Promise<{ default: infer C }>
  ? C
  : never;

/** Retries a dynamic modal import once so a transient Vite chunk race does not blank the launcher. */
function lazyModal<C extends LazyModalComponent>(loader: () => Promise<{ default: C }>) {
  return lazy<C>(async () => {
    try {
      return await loader();
    } catch {
      return await loader();
    }
  });
}

interface ModalBoundaryProps {
  resetKey: string;
  onError: () => void;
  children: ReactNode;
}

/** Isolates modal chunk/render failures so a broken dialog never unmounts the launcher. */
class ModalBoundary extends Component<ModalBoundaryProps, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError(): { failed: boolean } {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Modal boundary caught render failure:", error, info.componentStack);
    this.props.onError();
  }

  componentDidUpdate(prevProps: ModalBoundaryProps): void {
    if (this.state.failed && prevProps.resetKey !== this.props.resetKey) {
      this.setState({ failed: false });
    }
  }

  render(): ReactNode {
    return this.state.failed ? null : this.props.children;
  }
}

const AccountDialog = lazyModal(() =>
  import("./AccountDialog").then((m) => ({ default: m.AccountDialog })),
);
const CreateProfileDialog = lazyModal(() =>
  import("./CreateProfileDialog").then((m) => ({ default: m.CreateProfileDialog })),
);
const LaunchProblemDialog = lazyModal(() =>
  import("./LaunchProblemDialog").then((m) => ({ default: m.LaunchProblemDialog })),
);
const OnboardingDialog = lazyModal(() =>
  import("./OnboardingDialog").then((m) => ({ default: m.OnboardingDialog })),
);
const PrepareDialog = lazyModal(() =>
  import("./PrepareDialog").then((m) => ({ default: m.PrepareDialog })),
);
const ProfileDialog = lazyModal(() =>
  import("./ProfileDialog").then((m) => ({ default: m.ProfileDialog })),
);
const RemoveProfileDialog = lazyModal(() =>
  import("./RemoveProfileDialog").then((m) => ({ default: m.RemoveProfileDialog })),
);
const RemovedProfilesDialog = lazyModal(() =>
  import("./RemovedProfilesDialog").then((m) => ({ default: m.RemovedProfilesDialog })),
);
const SettingsDialog = lazyModal(() =>
  import("./SettingsDialog").then((m) => ({ default: m.SettingsDialog })),
);
const SignInDialog = lazyModal(() =>
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
  | { kind: "removed-profiles" }
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
  const refreshSeqRef = useRef(0);
  const accountsSeqRef = useRef(0);
  const selectAccountSeqRef = useRef(0);
  const switchTimerRef = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (switchTimerRef.current !== null) window.clearTimeout(switchTimerRef.current);
    },
    [],
  );
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
    const seq = ++refreshSeqRef.current;
    api.list().then(
      (snap) => {
        if (refreshSeqRef.current === seq) setSnapshot(snap);
      },
      () => {
        if (refreshSeqRef.current === seq) setSnapshot(EMPTY);
      },
    ); // no backend, no profiles: nothing is invented
  }, [api]);
  useEffect(refresh, [refresh]);

  const refreshAccounts = useCallback(() => {
    const seq = ++accountsSeqRef.current;
    accountsClient
      .list()
      .then(
        (snap) => {
          if (accountsSeqRef.current === seq) setAccounts(snap);
        },
        () => {
          if (accountsSeqRef.current === seq) setAccounts(NO_ACCOUNTS);
        },
      ) // no backend, no accounts: nothing is invented
      .finally(() => {
        if (accountsSeqRef.current === seq) setAccountsLoaded(true);
      });
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
    const seenFromEvent = new Set<string>();
    void launchClient
      .onState((ev) => {
        if (stale) return;
        seenFromEvent.add(ev.instanceId);
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
            setSurface((cur) => (cur.kind === "none" ? cur : { kind: "none" }));
            setProblem({ kind: "ended", profileId: ev.instanceId, exit: ev.exit });
          }
        }
      })
      .then((u) => {
        if (stale) u();
        else off = u;
      });
    launchClient.status().then(
      (list) => {
        if (stale) return;
        setRuns((prev) => {
          const next = { ...prev };
          for (const r of list) {
            if (!seenFromEvent.has(r.instanceId)) next[r.instanceId] = r.state;
          }
          return next;
        });
      },
      () => undefined, // no backend, nothing is running
    );
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
      const x = Math.round(rect.left * dpr);
      const y = Math.round(rect.top * dpr);
      const width = Math.max(0, Math.round(rect.right * dpr) - x);
      const height = Math.max(0, Math.round(rect.bottom * dpr) - y);
      if (width < 64 || height < 64) return;
      const next = { x, y, width, height, radius: 0 };
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
    const seq = ++selectAccountSeqRef.current;
    setAccounts((a) => ({ ...a, active: id }));
    accountsClient.select(id).then(
      (snap) => {
        if (selectAccountSeqRef.current === seq) setAccounts(snap);
      },
      () => {
        if (selectAccountSeqRef.current === seq) refreshAccounts();
      },
    );
  };

  // "+ Add account" and "Sign in again" both open the sign-in modal; the flow lives there.
  const addAccount = () => setSurface({ kind: "sign-in", accountId: null });

  const select = (id: string) => {
    setSnapshot((s) => ({ ...s, selected: id })); // immediate; the backend remembers it in data/settings.json
    api.select(id).catch(refresh);
  };

  const close = () => {
    if (switchTimerRef.current !== null) {
      window.clearTimeout(switchTimerRef.current);
      switchTimerRef.current = null;
    }
    setSurface({ kind: "none" });
  };

  const switchSurface = useCallback((next: Surface) => {
    if (switchTimerRef.current !== null) window.clearTimeout(switchTimerRef.current);
    setSurface({ kind: "none" });
    switchTimerRef.current = window.setTimeout(() => {
      switchTimerRef.current = null;
      setSurface(next);
    }, 0);
  }, []);

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
        void launchClient
          .status()
          .then((list) => {
            const cur = list.find((item) => item.instanceId === id);
            if (cur) setRuns((r) => ({ ...r, [id]: cur.state }));
          })
          .catch(() => undefined);
      } else {
        setSurface((cur) => (cur.kind === "none" ? cur : { kind: "none" }));
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
  const removed = snapshot.removed ?? [];
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
          aria-hidden={!embed.active}
          className={
            embed.active
              ? "pointer-events-none absolute inset-x-0 top-[44px] bottom-0"
              : "pointer-events-none invisible absolute inset-x-0 top-[44px] bottom-0"
          }
        >
          <div
            ref={stageRef}
            data-testid="game-stage"
            aria-hidden={!embed.active}
            className="h-full w-full"
          />
        </div>
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
        {!embed.active &&
          snapshot.profiles.length === 0 &&
          unreadable.length === 0 &&
          removed.length > 0 && (
            <div
              role="status"
              className="dock-rise pointer-events-none absolute inset-x-0 bottom-[calc(clamp(1.5rem,6vh,3.5rem)+4.625rem)] flex justify-center px-6"
            >
              <div className="bg-sand/75 text-graphite ring-graphite/10 pointer-events-auto flex max-w-[min(42rem,100%)] flex-wrap items-center justify-center gap-x-3 gap-y-1 rounded-2xl px-3.5 py-2 text-center text-[0.8125rem] leading-snug ring-1">
                <span className="text-ink-muted">
                  {t("removedProfiles.emptyNotice", { count: removed.length })}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    playSound("click");
                    setSurface({ kind: "removed-profiles" });
                  }}
                  className="text-graphite decoration-graphite/35 hover:text-clay-deep hover:decoration-clay-deep focus-visible:ring-clay/45 cursor-default rounded-md font-medium underline underline-offset-3 transition-colors outline-none focus-visible:ring-2"
                >
                  {t("removedProfiles.openAction")}
                </button>
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
            removedCount={removed.length}
            onOpenRemovedProfiles={() => setSurface({ kind: "removed-profiles" })}
            onPlay={play}
            playState={playState}
            onOpenSettings={() => setSurface({ kind: "settings" })}
          />
        )}
        <ModalBoundary
          resetKey={`${surface.kind}:${introduction ? 1 : 0}:${problem ? problem.kind : ""}`}
          onError={() => {
            setMounted({});
            setSurface({ kind: "none" });
            setProblem(null);
            setIntroduction(false);
          }}
        >
          <Suspense fallback={null}>
            {mounted.onboarding && (
              <OnboardingDialog open={introduction} onExit={endIntroduction} />
            )}
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
                  switchSurface({ kind: "prepare", id: editingProfile.id, verify: true })
                }
                onRemove={() =>
                  editingProfile && switchSurface({ kind: "remove-profile", id: editingProfile.id })
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
            {mounted["removed-profiles"] && (
              <RemovedProfilesDialog
                open={surface.kind === "removed-profiles"}
                removed={removed}
                onClose={close}
                onRestored={setSnapshot}
                api={api}
              />
            )}
            {mounted.problem && (
              <LaunchProblemDialog
                problem={problem}
                onClose={() => setProblem(null)}
                onRepair={(id) => {
                  setProblem(null);
                  switchSurface({ kind: "prepare", id });
                }}
                onSignIn={(accountId) => {
                  setProblem(null);
                  switchSurface({ kind: "sign-in", accountId });
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
                onSignInAgain={(id) => switchSurface({ kind: "sign-in", accountId: id })}
                onRemoved={(snapshot) => {
                  setAccounts(snapshot);
                  close();
                }}
                onVaultReset={setAccounts}
                api={accountsClient}
              />
            )}
          </Suspense>
        </ModalBoundary>
      </main>
    </TooltipProvider>
  );
}
