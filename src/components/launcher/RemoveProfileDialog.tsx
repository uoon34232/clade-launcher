import { useEffect, useRef, useState } from "react";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import type { ProfileSummary, ProfilesApi, ProfilesSnapshot } from "@/lib/profiles/api";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_QUIET, BUTTON_SOFT } from "./modalStyles";

interface Props {
  /** The profile to remove; null keeps the dialog closed. */
  profile: ProfileSummary | null;
  onClose: () => void;
  /** The backend moved the profile away and answered with the new list and selection. */
  onRemoved: (snapshot: ProfilesSnapshot) => void;
  api: ProfilesApi;
}

/**
 * "Remove from Clade…": a plain confirmation. Nothing is deleted: the backend moves the profile's
 * whole folder to a retained place inside the Clade folder, and the text says exactly that.
 */
export function RemoveProfileDialog({ profile, onClose, onRemoved, api }: Props) {
  // Keep the last profile while the closing animation runs.
  const [shown, setShown] = useState(profile);
  const [wasOpen, setWasOpen] = useState(profile !== null);
  const [busy, setBusy] = useState(false);
  const isOpen = profile !== null;
  if (profile && profile !== shown) setShown(profile);
  if (isOpen !== wasOpen) {
    setWasOpen(isOpen);
    setBusy(false);
  }
  const current = profile ?? shown;
  return (
    <Modal
      open={isOpen}
      onOpenChange={(o) => !o && onClose()}
      size="sm"
      locked={busy}
      closeOnBackdrop={false}
    >
      {current && (
        <Body
          key={current.id}
          profile={current}
          busy={busy}
          setBusy={setBusy}
          onClose={onClose}
          onRemoved={onRemoved}
          api={api}
        />
      )}
    </Modal>
  );
}

function Body({
  profile,
  busy,
  setBusy,
  onClose,
  onRemoved,
  api,
}: Omit<Props, "profile"> & {
  profile: ProfileSummary;
  busy: boolean;
  setBusy: (busy: boolean) => void;
}) {
  const t = useT();
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const remove = () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    playSound("click");
    api.remove(profile.id).then(
      (snapshot) => {
        if (!mountedRef.current) return;
        setBusy(false);
        onRemoved(snapshot);
      },
      (e: unknown) => {
        if (!mountedRef.current) return;
        setBusy(false);
        setError(describeError("profile", e));
      },
    );
  };

  return (
    <>
      <ModalHeader title={t("removeProfile.title", { profile: profile.name })} />
      <p className="text-graphite mb-5 text-[0.9375rem] leading-relaxed">
        {t("removeProfile.body")}
      </p>
      {error && (
        <p role="alert" className="text-clay-deep mt-3 text-[0.875rem]">
          {error}
        </p>
      )}
      <ModalFooter>
        <button type="button" autoFocus disabled={busy} className={BUTTON_QUIET} onClick={onClose}>
          {t("common.cancel")}
        </button>
        <button type="button" disabled={busy} className={BUTTON_SOFT} onClick={remove}>
          {busy ? t("removeProfile.working") : t("removeProfile.confirm")}
        </button>
      </ModalFooter>
    </>
  );
}
