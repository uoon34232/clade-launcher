import { SettingsIcon } from "lucide-react";
import { useT } from "@/lib/i18n";
import { playSound } from "@/lib/sound/soundService";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface Props {
  onClick?: () => void;
}

// 48px hit area matches the other dock items; the circle only appears on hover.
export function SettingsButton({ onClick }: Props) {
  const t = useT();
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <button
            type="button"
            onClick={() => {
              playSound("click");
              onClick?.();
            }}
            aria-label={t("dock.settings")}
            className="dock-item group text-cream/55 hover:bg-cream/[0.07] hover:text-cream focus-visible:ring-clay-soft grid size-12 cursor-default place-items-center rounded-full outline-none focus-visible:ring-2"
          />
        }
      >
        <SettingsIcon
          aria-hidden
          strokeWidth={1.75}
          className="settings-gear ease-out-soft size-5 transition-transform duration-300 group-hover:rotate-[30deg]"
        />
      </TooltipTrigger>
      <TooltipContent side="top" sideOffset={14}>
        {t("dock.settings")}
      </TooltipContent>
    </Tooltip>
  );
}
