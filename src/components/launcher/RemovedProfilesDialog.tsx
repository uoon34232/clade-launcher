import { useState } from "react";
import { formatPlaytime, profileDetail } from "@/features/launcher/profileUi";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import type { ProfilesApi, ProfilesSnapshot, RemovedProfileSummary } from "@/lib/profiles/api";
import { playSound } from "@/lib/sound/soundService";
import { cn } from "@/lib/utils";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_QUIET, BUTTON_SMALL_SOFT, BUTTON_SOFT } from "./modalStyles";

interface Props {
  open: boolean;
  removed: readonly RemovedProfileSummary[];
  onClose: () => void;
  onRestored: (snapshot: ProfilesSnapshot) => void;
  api: ProfilesApi;
}

/**
 * Lists profiles moved to `instances/.removed/` and lets the player restore any of them in one
 * click or open the folder in File Explorer to clean up files manually. Clade never erases files.
 */
export function RemovedProfilesDialog({ open, removed, onClose, onRestored, api }: Props) {
  const t = useT();
  const [busyFolder, setBusyFolder] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const handleOpenChange = (next: boolean) => {
    if (!next && busyFolder === null) {
      setError(null);
      onClose();
    }
  };

  const restoreOne = (folder: string) => {
    if (!api.restore || busyFolder !== null) return;
    setBusyFolder(folder);
    setError(null);
    playSound("click");
    api.restore(folder).then(
      (snapshot) => {
        setBusyFolder(null);
        onRestored(snapshot);
        if ((snapshot.removed ?? []).length === 0) onClose();
      },
      (e: unknown) => {
        setBusyFolder(null);
        setError(describeError("profile", e));
      },
    );
  };

  const openFolder = () => {
    if (!api.openRemovedFolder) return;
    setError(null);
    playSound("click");
    api.openRemovedFolder().catch((e: unknown) => setError(describeError("profile", e)));
  };

  return (
    <Modal open={open} onOpenChange={handleOpenChange} size="md" locked={busyFolder !== null}>
      <ModalHeader
        title={t("removedProfiles.title")}
        description={t("removedProfiles.description")}
      />
      <div className="mb-5">
        {removed.length === 0 ? (
          <p className="text-ink-muted py-3 text-[0.9375rem]">{t("removedProfiles.empty")}</p>
        ) : (
          <ul className="quiet-scroll divide-graphite/10 border-graphite/10 bg-sand/30 max-h-64 divide-y overflow-y-auto rounded-2xl border">
            {removed.map((item) => {
              const busy = busyFolder === item.folder;
              const detail =
                item.playtimeSeconds >= 60
                  ? `${profileDetail(item)} · ${formatPlaytime(item.playtimeSeconds)}`
                  : profileDetail(item);
              return (
                <li key={item.folder} className="flex items-center justify-between gap-3 px-4 py-3">
                  <div className="min-w-0">
                    <p className="text-graphite truncate text-[0.9375rem] font-medium">
                      {item.name}
                    </p>
                    <p className="text-ink-muted truncate text-[0.8125rem]">{detail}</p>
                  </div>
                  <button
                    type="button"
                    disabled={busyFolder !== null}
                    onClick={() => restoreOne(item.folder)}
                    className={`${BUTTON_SMALL_SOFT} shrink-0`}
                  >
                    {busy ? t("removedProfiles.restoring") : t("removedProfiles.restore")}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
      {error && (
        <p role="alert" className="text-clay-deep mt-3 text-[0.875rem]">
          {error}
        </p>
      )}
      <ModalFooter>
        {api.openRemovedFolder && (
          <button
            type="button"
            disabled={busyFolder !== null}
            onClick={openFolder}
            className={cn(BUTTON_QUIET, "mr-auto px-3")}
          >
            {t("removedProfiles.openFolder")}
          </button>
        )}
        <button
          type="button"
          disabled={busyFolder !== null}
          onClick={() => {
            playSound("click");
            onClose();
          }}
          className={BUTTON_SOFT}
        >
          {t("common.done")}
        </button>
      </ModalFooter>
    </Modal>
  );
}
