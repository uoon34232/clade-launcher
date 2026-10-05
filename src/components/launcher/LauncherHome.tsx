import { useCallback, useEffect, useState } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  accountsApi,
  NO_ACCOUNTS,
  type AccountsApi,
  type AccountsSnapshot,
} from "@/lib/accounts/api";
import { launchApi, LaunchRefused, type LaunchApi, type RunState } from "@/lib/launch/api";
import { profilesApi, type ProfilesApi, type ProfilesSnapshot } from "@/lib/profiles/api";
import { completeOnboarding, type UiSettings } from "@/lib/settings/uiSettings";
import { playSound } from "@/lib/sound/soundService";
import { AnimatedBackground } from "./AnimatedBackground";
import { AccountDialog } from "./AccountDialog";
import { CreateProfileDialog } from "./CreateProfileDialog";
import { Hero } from "./Hero";
import { LaunchProblemDialog, type Problem } from "./LaunchProblemDialog";
import { LauncherDock } from "./LauncherDock";
import { OnboardingDialog, type OnboardingExit } from "./OnboardingDialog";
import type { PlayState } from "./PlayButton";
import { PrepareDialog } from "./PrepareDialog";
import { SettingsDialog } from "./SettingsDialog";
import { SignInDialog } from "./SignInDialog";
import { WindowChrome } from "./WindowChrome";

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
  | { kind: "settings" }
  | { kind: "prepare"; id: string; verify?: boolean }
  | { kind: "sign-in"; accountId: string | null }
  | { kind: "account"; id: string };

const EMPTY: ProfilesSnapshot = { profiles: [], selected: null, totalPlaytimeSeconds: 0 };

export function LauncherHome({
  initialUi,
  api = profilesApi,
  accountsClient = accountsApi,
  launchClient = launchApi,
}: Props) {
  const [snapshot, setSnapshot] = useState<ProfilesSnapshot>(EMPTY);
  const [accounts, setAccounts] = useState<AccountsSnapshot>(NO_ACCOUNTS);
  const [surface, setSurface] = useState<Surface>({ kind: "none" });
  const [runs, setRuns] = useState<Record<string, RunState>>({});
  const [problem, setProblem] = useState<Problem | null>(null);
  // The first-run introduction: only when the backend says it was never finished or skipped.
  const [introduction, setIntroduction] = useState(initialUi?.onboardingDone === false);

  const refresh = useCallback(() => {
    api.list().then(setSnapshot, () => setSnapshot(EMPTY)); // no backend, no profiles: nothing is invented
  }, [api]);
  useEffect(refresh, [refresh]);

  const refreshAccounts = useCallback(() => {
    accountsClient.list().then(setAccounts, () => setAccounts(NO_ACCOUNTS)); // no backend, no accounts: nothing is invented
  }, [accountsClient]);
  useEffect(refreshAccounts, [refreshAccounts]);

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
          refresh(); // the playtime moved
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
  }, [launchClient, refresh]);

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

  // The primary action: set the profile up, or play it; with none, start creating one.
  const play = () => {
    if (!selectedProfile) setSurface({ kind: "create-profile" });
    else if (!selectedProfile.ready) setSurface({ kind: "prepare", id: selectedProfile.id });
    else launch(selectedProfile.id);
  };

  const prepareSurface = surface.kind === "prepare" ? surface : null;
  const preparing = prepareSurface
    ? (snapshot.profiles.find((p) => p.id === prepareSurface.id) ?? null)
    : null;
  const run = selectedProfile ? runs[selectedProfile.id] : undefined;
  const playState: PlayState =
    run ??
    (preparing && selectedProfile && preparing.id === selectedProfile.id && preparing.ready
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

  return (
    <TooltipProvider delay={350}>
      <main className="relative h-full w-full overflow-hidden">
        <AnimatedBackground />
        <WindowChrome />
        <section className="absolute inset-0 grid place-items-center pb-[clamp(6rem,19vh,12rem)]">
          <Hero
            lastPhraseId={initialUi?.lastPhrase}
            playtimeSeconds={snapshot.totalPlaytimeSeconds}
          />
        </section>
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
          onVerifyProfile={() =>
            selectedProfile && setSurface({ kind: "prepare", id: selectedProfile.id, verify: true })
          }
          verifyDisabled={run !== undefined}
          onPlay={play}
          playState={playState}
          onOpenSettings={() => setSurface({ kind: "settings" })}
        />
        <OnboardingDialog open={introduction} onExit={endIntroduction} />
        <CreateProfileDialog
          open={surface.kind === "create-profile"}
          onOpenChange={(o) => !o && close()}
          existingNames={snapshot.profiles.map((p) => p.name)}
          onCreated={setSnapshot}
          api={api}
        />
        <PrepareDialog
          profile={preparing}
          verify={prepareSurface?.verify === true}
          onClose={close}
          onPrepared={setSnapshot}
          api={api}
        />
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
          accountsClient={accountsClient}
        />
        <SettingsDialog open={surface.kind === "settings"} onOpenChange={(o) => !o && close()} />
        <SignInDialog
          open={surface.kind === "sign-in"}
          onOpenChange={(o) => !o && close()}
          account={signingIn ? { id: signingIn.id, username: signingIn.username } : null}
          onSignedIn={(snapshot) => {
            setAccounts(snapshot);
            close();
          }}
          api={accountsClient}
        />
        <AccountDialog
          account={managed}
          capabilities={accounts.capabilities}
          onClose={close}
          onSignInAgain={(id) => setSurface({ kind: "sign-in", accountId: id })}
          onRemoved={(snapshot) => {
            setAccounts(snapshot);
            close();
          }}
          api={accountsClient}
        />
      </main>
    </TooltipProvider>
  );
}
