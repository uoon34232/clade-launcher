// How a preparation stage is worded. The backend reports stages by id; the wording is the UI's, so
// it follows the selected language (Rust never sends finished prose).

import { LOADER_NAMES, type LoaderKind, type PrepareStage } from "@/lib/profiles/api";
import { t, type MessageKey } from "@/lib/i18n";

const STAGE_KEYS: Readonly<Record<PrepareStage, MessageKey>> = {
  planning: "prepare.stage.planning",
  java: "prepare.stage.java",
  minecraft: "prepare.stage.minecraft",
  libraries: "prepare.stage.libraries",
  assets: "prepare.stage.assets",
  natives: "prepare.stage.natives",
  loader: "prepare.stage.loader",
  verifying: "prepare.stage.verifying",
  done: "prepare.stage.done",
};

/** Stages that say "Verifying ..." instead of "Downloading ..." during a full verification run. */
const VERIFY_KEYS: Partial<Readonly<Record<PrepareStage, MessageKey>>> = {
  minecraft: "prepare.stage.minecraft.verify",
  libraries: "prepare.stage.libraries.verify",
  assets: "prepare.stage.assets.verify",
};

/** "Downloading libraries", "Verifying assets", "Installing Fabric", ... (without the ellipsis). */
export function stageText(
  stage: PrepareStage | null,
  options: { verify: boolean; loader: LoaderKind | null },
): string {
  const s = stage ?? "planning";
  const key = (options.verify ? VERIFY_KEYS[s] : undefined) ?? STAGE_KEYS[s];
  return t(key, { loader: options.loader ? LOADER_NAMES[options.loader] : "" });
}
