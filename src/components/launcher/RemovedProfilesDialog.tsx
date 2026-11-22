import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
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
  const listId = useId();
  const [busyFolder, setBusyFolder] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);
  const mountedRef = useRef(true);
  const openRef = useRef(open);
  const sessionRef = useRef(0);
  const openFolderInFlightRef = useRef(false);
  const listRef = useRef<HTMLUListElement>(null);
  const pendingRestoreFocusIndexRef = useRef<number | null>(null);

  if (open !== wasOpen) {
    setWasOpen(open);
    setBusyFolder(null);
    setError(null);
  }

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!open || busyFolder !== null || pendingRestoreFocusIndexRef.current === null) return;
    const idx = pendingRestoreFocusIndexRef.current;
    pendingRestoreFocusIndexRef.current = null;
    const activeEl = document.activeElement;
    if (activeEl && activeEl !== document.body && activeEl !== document.documentElement) return;
    const buttons = listRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
    if (buttons && buttons.length > 0) {
      buttons[Math.min(idx, buttons.length - 1)]?.focus();
    }
  }, [open, busyFolder, removed.length]);

  useLayoutEffect(() => {
    openRef.current = open;
    sessionRef.current += 1;
    if (!open) {
      openFolderInFlightRef.current = false;
      pendingRestoreFocusIndexRef.current = null;
    }
  }, [open]);

  const close = () => {
    sessionRef.current += 1;
    setError(null);
    onClose();
  };

  const handleOpenChange = (next: boolean) => {
    if (!next && busyFolder === null) {
      close();
    }
  };

  const restoreOne = (folder: string, index: number) => {
    if (!api.restore || busyFolder !== null || !open) return;
    const session = sessionRef.current;
    pendingRestoreFocusIndexRef.current = index;
    setBusyFolder(folder);
    setError(null);
    playSound("click");
    api.restore(folder).then(
      (snapshot) => {
        if (!mountedRef.current) return;
        const sameSession = openRef.current && sessionRef.current === session;
        if (!sameSession) {
          onRestored(snapshot);
          return;
        }
        setBusyFolder(null);
        onRestored(snapshot);
        if ((snapshot.removed ?? []).length === 0) close();
      },
      (e: unknown) => {
        if (!mountedRef.current || !openRef.current || sessionRef.current !== session) return;
        setBusyFolder(null);
        setError(describeError("profile", e));
      },
    );
  };

  const openFolder = () => {
    if (!api.openRemovedFolder || !open || openFolderInFlightRef.current) return;
    openFolderInFlightRef.current = true;
    const session = sessionRef.current;
    setError(null);
    playSound("click");
    api.openRemovedFolder().then(
      () => {
        if (sessionRef.current === session) openFolderInFlightRef.current = false;
      },
      (e: unknown) => {
        if (sessionRef.current === session) openFolderInFlightRef.current = false;
        if (!mountedRef.current || !openRef.current || sessionRef.current !== session) return;
        setError(describeError("profile", e));
      },
    );
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
          <ul
            ref={listRef}
            className="quiet-scroll divide-graphite/10 border-graphite/10 bg-sand/30 max-h-64 divide-y overflow-y-auto rounded-2xl border forced-colors:border-[CanvasText]"
          >
            {removed.map((item, index) => {
              const busy = busyFolder === item.folder;
              const nameId = `${listId}-name-${index}`;
              const detail =
                item.playtimeSeconds >= 60
                  ? `${profileDetail(item)} · ${formatPlaytime(item.playtimeSeconds)}`
                  : profileDetail(item);
              return (
                <li key={item.folder} className="flex items-center justify-between gap-3 px-4 py-3">
                  <div className="min-w-0">
                    <p id={nameId} className="text-graphite truncate text-[0.9375rem] font-medium">
                      {item.name}
                    </p>
                    <p className="text-ink-muted truncate text-[0.8125rem]">{detail}</p>
                  </div>
                  <button
                    type="button"
                    disabled={busyFolder !== null}
                    aria-describedby={nameId}
                    onClick={() => restoreOne(item.folder, index)}
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
            if (busyFolder !== null) return;
            playSound("click");
            close();
          }}
          className={BUTTON_SOFT}
        >
          {t("common.done")}
        </button>
      </ModalFooter>
    </Modal>
  );
}
