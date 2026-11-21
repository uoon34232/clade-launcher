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
  onOpenProfileSettings: () => void;
  onOpenProfileFolder: () => void;
  onPlay: () => void;
  /** What the primary button shows for the selected profile. */
  playState: PlayState;
  onVerifyProfile: () => void;
  /** The selected profile's game is starting or running, so "Verify files" is unavailable. */
  verifyDisabled?: boolean;
  /** Remove the selected profile from Clade (its files are kept). */
  onRemoveProfile: () => void;
  removeDisabled?: boolean;
  removedCount?: number;
  onOpenRemovedProfiles?: () => void;
  onOpenSettings: () => void;
  /** True right after an embedded game closes so the dock rises back swiftly. */
  returning?: boolean;
}

export function LauncherDock(p: Props) {
  return (
    <div
      className={`launcher-dock-wrap ${p.returning ? "dock-return" : "dock-rise"} absolute inset-x-0 bottom-[clamp(1.5rem,6vh,3.5rem)] z-[15] flex justify-center px-6`}
    >
      <nav aria-label="Clade" className="glass-dock flex items-center gap-2 rounded-[1.75rem] p-2">
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
          onOpenSettings={p.onOpenProfileSettings}
          onOpenFolder={p.onOpenProfileFolder}
          onVerify={p.onVerifyProfile}
          verifyDisabled={p.verifyDisabled}
          onRemove={p.onRemoveProfile}
          removeDisabled={p.removeDisabled}
          removedCount={p.removedCount}
          onOpenRemoved={p.onOpenRemovedProfiles}
        />
        <PlayButton onClick={p.onPlay} state={p.playState} />
        <SettingsButton onClick={p.onOpenSettings} />
      </nav>
    </div>
  );
}
