import { useState } from "react";
import { skinSize, type Account } from "@/lib/accounts/api";
import { PixelAvatar } from "./PixelAvatar";

/**
 * The account's Minecraft head, cut out of its current skin texture: the face layer, then the
 * outer hat layer over it (transparent where the skin has none). Both are 8x8 cells of the
 * texture (face at 8,8; hat at 40,8; scaled for high-resolution skins) shown nearest-neighbour,
 * never smoothed. With no skin, or one the browser cannot decode, the neutral face is drawn.
 */
export function PlayerHead({
  account,
  className,
}: {
  account?: Pick<Account, "skin" | "skinDimensions" | "username">;
  className?: string;
}) {
  const skin = account?.skin ?? null;
  const size = skin
    ? account?.skinDimensions !== undefined
      ? account.skinDimensions
      : skinSize(skin)
    : null;
  // The texture a failed decode belongs to, so a later (different) skin gets its own chance.
  const [broken, setBroken] = useState<string | null>(null);
  if (!skin || !size || broken === skin) return <PixelAvatar className={className} />;

  const scale = size.width / 64;
  const cell = 8 * scale;
  const cut = (x: number, key: string, onError?: () => void) => (
    <svg
      key={key}
      x="0"
      y="0"
      width="8"
      height="8"
      viewBox={`${x * scale} ${cell} ${cell} ${cell}`}
    >
      <image
        href={skin}
        x="0"
        y="0"
        width={size.width}
        height={size.height}
        preserveAspectRatio="none"
        imageRendering="pixelated"
        style={{ imageRendering: "pixelated" }}
        data-layer={key}
        onError={onError}
      />
    </svg>
  );
  return (
    <svg
      viewBox="0 0 8 8"
      shapeRendering="crispEdges"
      className={`${className ?? ""} [image-rendering:pixelated]`}
      aria-hidden
      data-skin-head=""
    >
      {cut(8, "face", () => setBroken(skin))}
      {cut(40, "hat")}
    </svg>
  );
}
