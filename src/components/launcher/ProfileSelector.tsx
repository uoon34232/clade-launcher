import { ChevronUpIcon } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { profileDetail, profileLabel } from "@/features/launcher/profileUi";
import type { ProfileSummary } from "@/lib/profiles/api";
import { useT } from "@/lib/i18n";
import { playSound } from "@/lib/sound/soundService";

interface Props {
  profiles: readonly ProfileSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onOpenSettings: () => void;
  onOpenFolder: () => void;
  /** Re-checks every file of the selected profile (a full hash verification) and repairs what differs. */
  onVerify: () => void;
  /** The selected profile's game is starting or running: its files must not be touched now. */
  verifyDisabled?: boolean;
  /** Starts removing the SELECTED profile from Clade (after a confirmation); its files are kept. */
  onRemove: () => void;
  /** Its game is starting or running: it cannot be moved now. (An account is not needed to remove it.) */
  removeDisabled?: boolean;
  /** Number of retained profiles in `instances/.removed/`. */
  removedCount?: number;
  /** Opens the dialog to restore retained profiles or open their folder. */
  onOpenRemoved?: () => void;
}

const TRIGGER =
  "dock-item group flex h-12 min-w-[clamp(11rem,17vw,14.5rem)] max-w-[min(21rem,36vw)] cursor-default items-center justify-between gap-5 rounded-[1.25rem] bg-cream/[0.065] pr-3 pl-4 text-left outline-none hover:bg-cream/[0.11] focus-visible:ring-2 focus-visible:ring-clay-soft data-popup-open:bg-cream/[0.13]";

/**
 * The dock's profile control: shows the selected profile ("Survival"), lists the existing profiles,
 * and keeps profile actions pinned below the scrolling list. With no profiles at all it is simply a
 * button that starts creating the first one.
 */
export function ProfileSelector({
  profiles,
  selectedId,
  onSelect,
  onCreate,
  onOpenSettings,
  onOpenFolder,
  onVerify,
  verifyDisabled = false,
  onRemove,
  removeDisabled = false,
  removedCount = 0,
  onOpenRemoved,
}: Props) {
  const t = useT();
  const current = profiles.find((p) => p.id === selectedId) ?? profiles[0];

  if (!current) {
    return (
      <button
        type="button"
        className={TRIGGER}
        onClick={() => {
          playSound("click");
          onCreate();
        }}
      >
        <span className="text-cream -translate-y-px text-[0.9375rem] leading-none font-medium tracking-[0.005em]">
          {t("profile.createFirst")}
        </span>
        <span aria-hidden className="text-cream/55 text-[1.125rem] leading-none">
          +
        </span>
      </button>
    );
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        onClick={() => playSound("click")}
        aria-label={t("profile.trigger", { label: profileLabel(current) })}
        className={TRIGGER}
      >
        <span className="text-cream -translate-y-px truncate text-[0.9375rem] leading-[1.4] font-medium tracking-[0.005em]">
          {profileLabel(current)}
        </span>
        <ChevronUpIcon
          aria-hidden
          strokeWidth={1.75}
          className="text-cream/50 ease-out-soft group-hover:text-cream/85 group-data-popup-open:text-cream/85 size-4 shrink-0 transition-[transform,color] duration-200 group-data-popup-open:rotate-180"
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        align="center"
        sideOffset={14}
        className="glass-menu flex max-h-[min(22rem,var(--available-height))] w-[max(var(--anchor-width),18rem)] flex-col overflow-hidden rounded-[1.25rem] p-1.5 duration-200"
      >
        {/* Only the list of profiles scrolls; profile actions stay in view below it. */}
        <div className="menu-scroll min-h-0 flex-1 overflow-y-auto">
          <DropdownMenuRadioGroup
            value={current.id}
            onValueChange={(id: string) => {
              playSound("select");
              onSelect(id);
            }}
          >
            {profiles.map((p) => (
              <DropdownMenuRadioItem
                key={p.id}
                value={p.id}
                closeOnClick
                className="text-cream/80 data-[checked]:bg-cream/[0.06] data-[checked]:text-cream focus:bg-cream/[0.1] focus:text-cream [&_[data-slot=dropdown-menu-radio-item-indicator]]:text-clay-soft min-h-11 rounded-[0.875rem] py-1.5 pr-9 pl-3 transition-colors duration-150"
              >
                <span className="flex min-w-0 flex-col leading-tight">
                  <span className="truncate text-[0.875rem] font-medium">{p.name}</span>
                  <span className="text-cream/55 truncate text-[0.75rem]">{profileDetail(p)}</span>
                </span>
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </div>
        <DropdownMenuSeparator className="bg-cream/[0.08] my-1 shrink-0" />
        <DropdownMenuItem
          className="text-cream/80 focus:bg-cream/[0.1] focus:text-cream min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] font-medium transition-colors duration-150"
          onClick={() => {
            playSound("click");
            // Let the menu finish closing (and return focus to its trigger) before the dialog takes focus.
            window.setTimeout(onCreate, 0);
          }}
        >
          {t("profile.create")}
        </DropdownMenuItem>
        <DropdownMenuItem
          className="text-cream/80 focus:bg-cream/[0.1] focus:text-cream min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] transition-colors duration-150"
          onClick={() => {
            playSound("click");
            window.setTimeout(onOpenSettings, 0);
          }}
        >
          {t("profile.settings")}
        </DropdownMenuItem>
        <DropdownMenuItem
          className="text-cream/65 focus:bg-cream/[0.1] focus:text-cream min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] transition-colors duration-150 data-disabled:opacity-40"
          onClick={() => {
            playSound("click");
            onOpenFolder();
          }}
        >
          {t("profile.openFolder")}
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={verifyDisabled}
          className="text-cream/65 focus:bg-cream/[0.1] focus:text-cream min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] transition-colors duration-150 data-disabled:opacity-40"
          onClick={() => {
            playSound("click");
            window.setTimeout(onVerify, 0);
          }}
        >
          {t("profile.verify")}
        </DropdownMenuItem>
        {removedCount > 0 && onOpenRemoved && (
          <DropdownMenuItem
            className="text-cream/65 focus:bg-cream/[0.1] focus:text-cream min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] transition-colors duration-150"
            onClick={() => {
              playSound("click");
              window.setTimeout(onOpenRemoved, 0);
            }}
          >
            {t("profile.removedMenu", { count: removedCount })}
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator className="bg-cream/[0.08] my-1 shrink-0" />
        <DropdownMenuItem
          disabled={removeDisabled}
          className="text-clay-soft focus:bg-clay/20 focus:text-clay-soft min-h-10 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] transition-colors duration-150 data-disabled:opacity-40"
          onClick={() => {
            playSound("click");
            window.setTimeout(onRemove, 0);
          }}
        >
          {t("profile.remove")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
