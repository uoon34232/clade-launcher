import { useState } from "react";
import { SettingsIcon } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Account } from "@/lib/accounts/api";
import { useT } from "@/lib/i18n";
import { playSound } from "@/lib/sound/soundService";
import { PlayerHead } from "./PlayerHead";

interface Props {
  accounts: readonly Account[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onManage: (id: string) => void;
  onAdd: () => void;
}

const TRIGGER =
  "dock-item group flex h-12 cursor-default items-center gap-2 rounded-[1.25rem] pr-3.5 pl-2 outline-none hover:bg-cream/[0.07] focus-visible:ring-2 focus-visible:ring-clay-soft data-popup-open:bg-cream/[0.1]";
const ROW_ITEM =
  "min-h-11 gap-2.5 rounded-[0.875rem] py-1.5 pl-2.5 text-cream/80 transition-colors duration-150 data-[checked]:text-cream focus:bg-cream/[0.1] focus:text-cream [&_[data-slot=dropdown-menu-radio-item-indicator]]:text-clay-soft";

/**
 * The dock's account control: the active account (Minecraft head and Java profile name) and a
 * compact menu to switch. Accounts are independent from profiles: choosing one changes who plays,
 * never what is played. The manage icon appears on hover or keyboard focus.
 */
export function AccountMenu({ accounts, activeId, onSelect, onManage, onAdd }: Props) {
  const t = useT();
  const active = accounts.find((a) => a.id === activeId) ?? accounts[0];
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [focusedId, setFocusedId] = useState<string | null>(null);

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (!open) {
          setHoveredId(null);
          setFocusedId(null);
        }
      }}
    >
      <DropdownMenuTrigger
        onClick={() => playSound("click")}
        aria-label={
          active ? t("account.trigger", { name: active.username }) : t("account.triggerNone")
        }
        className={TRIGGER}
      >
        <PlayerHead
          account={active}
          className="ring-cream/10 size-8 shrink-0 rounded-[0.5rem] ring-1"
        />
        <span className="text-cream/75 group-hover:text-cream max-w-[9.5rem] -translate-y-px truncate text-[0.875rem] leading-tight font-medium tracking-[0.005em] transition-colors duration-200">
          {active ? active.username : t("account.signIn")}
        </span>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        align="start"
        sideOffset={14}
        className="glass-menu max-h-[min(22rem,var(--available-height))] w-[max(var(--anchor-width),16rem)] rounded-[1.25rem] p-1.5 duration-200"
      >
        {active && (
          <>
            <div
              role="presentation"
              className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-1"
              style={{ gridTemplateRows: `repeat(${accounts.length}, auto)` }}
            >
              <DropdownMenuRadioGroup
                value={active.id}
                onValueChange={(id: string) => {
                  playSound("select");
                  onSelect(id);
                }}
                className="col-start-1 grid"
                style={{
                  gridRow: `1 / span ${accounts.length}`,
                  gridTemplateRows: "subgrid",
                }}
              >
                {accounts.map((a) => (
                  <DropdownMenuRadioItem
                    key={a.id}
                    value={a.id}
                    closeOnClick
                    onPointerEnter={() => setHoveredId(a.id)}
                    onPointerLeave={() => setHoveredId((prev) => (prev === a.id ? null : prev))}
                    onFocus={() => setFocusedId(a.id)}
                    onBlur={() => setFocusedId((prev) => (prev === a.id ? null : prev))}
                    className={`${ROW_ITEM} data-[checked]:bg-cream/[0.06] min-w-0 pr-9`}
                  >
                    <PlayerHead
                      account={a}
                      className="ring-cream/10 size-7 shrink-0 rounded-[0.4rem] ring-1"
                    />
                    <span className="flex min-w-0 flex-col leading-tight">
                      <span className="truncate text-[0.875rem] font-medium">{a.username}</span>
                      {a.needsSignIn && (
                        <span className="text-cream/55 text-[0.75rem]">
                          {t("account.needsSignIn")}
                        </span>
                      )}
                    </span>
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
              {accounts.map((a, i) => {
                const revealed = hoveredId === a.id || focusedId === a.id;
                return (
                  <DropdownMenuItem
                    key={a.id}
                    style={{ gridColumnStart: 2, gridRowStart: i + 1 }}
                    aria-label={t("account.manage", { name: a.username })}
                    onPointerEnter={() => setHoveredId(a.id)}
                    onPointerLeave={() => setHoveredId((prev) => (prev === a.id ? null : prev))}
                    onFocus={() => setFocusedId(a.id)}
                    onBlur={() => setFocusedId((prev) => (prev === a.id ? null : prev))}
                    className={`text-cream/55 focus:bg-cream/[0.1] focus:text-cream size-9 shrink-0 justify-center rounded-[0.75rem] p-0 transition-[opacity,background-color,color] duration-150 hover:opacity-100 focus:opacity-100 data-[highlighted]:opacity-100 ${revealed ? "opacity-100" : "opacity-0"}`}
                    onClick={() => {
                      playSound("click");
                      window.setTimeout(() => onManage(a.id), 0); // after the menu has closed and returned focus
                    }}
                  >
                    <SettingsIcon aria-hidden strokeWidth={1.75} className="size-4" />
                  </DropdownMenuItem>
                );
              })}
            </div>
            <DropdownMenuSeparator className="bg-cream/[0.08] my-1.5" />
          </>
        )}
        <DropdownMenuItem
          className="text-cream/80 focus:bg-cream/[0.1] focus:text-cream min-h-11 rounded-[0.875rem] py-1.5 pl-3 text-[0.875rem] font-medium transition-colors duration-150"
          onClick={() => {
            playSound("click");
            window.setTimeout(onAdd, 0);
          }}
        >
          {t("account.add")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
