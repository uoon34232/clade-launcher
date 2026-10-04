import type { Account } from "@/lib/accounts/api";
import type { ProfileSummary } from "@/lib/profiles/api";
import { AccountMenu } from "./AccountMenu";
import { PlayButton, type PlayState } from "./PlayButton";
import { ProfileSelector } from "./ProfileSelector";
import { SettingsButton } from "./SettingsButton";

interface Props {
  accounts: readonly Account[];
  activeAccountId: string | null;
  onSelectAccount: (id: string) => void;
  onManageAccount: (id: string) => void;
  onAddAccount: () => void;
  profiles: readonly ProfileSummary[];
  selectedProfileId: string | null;
  onSelectProfile: (id: string) => void;
  onCreateProfile: () => void;
  onPlay: () => void;
  /** What the primary button shows for the selected profile. */
  playState: PlayState;
  onVerifyProfile: () => void;
  onOpenSettings: () => void;
}

export function LauncherDock(p: Props) {
  return (
    <div className="dock-rise absolute inset-x-0 bottom-[clamp(1.5rem,6vh,3.5rem)] flex justify-center px-6">
      <nav
        aria-label="Clade"
        className="bg-graphite/95 flex items-center gap-2 rounded-[1.75rem] p-2 shadow-[inset_0_1px_0_rgb(255_255_255/0.07),0_30px_70px_-24px_rgb(43_38_34/0.6),0_3px_10px_-2px_rgb(43_38_34/0.25)]"
      >
        <AccountMenu
          accounts={p.accounts}
          activeId={p.activeAccountId}
          onSelect={p.onSelectAccount}
          onManage={p.onManageAccount}
          onAdd={p.onAddAccount}
        />
        <ProfileSelector
          profiles={p.profiles}
          selectedId={p.selectedProfileId}
          onSelect={p.onSelectProfile}
          onCreate={p.onCreateProfile}
          onVerify={p.onVerifyProfile}
        />
        <PlayButton onClick={p.onPlay} state={p.playState} />
        <SettingsButton onClick={p.onOpenSettings} />
      </nav>
    </div>
  );
}
