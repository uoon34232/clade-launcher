import type { Account } from "@/lib/accounts/api";
import { PixelAvatar } from "./PixelAvatar";

/** The account's Minecraft head: the real one once the backend has rendered it, a neutral face until then. */
export function PlayerHead({
  account,
  className,
}: {
  account?: Pick<Account, "head" | "username">;
  className?: string;
}) {
  if (account?.head) {
    return (
      <img
        src={account.head}
        alt=""
        aria-hidden
        draggable={false}
        className={`${className ?? ""} [image-rendering:pixelated]`}
      />
    );
  }
  return <PixelAvatar className={className} />;
}
