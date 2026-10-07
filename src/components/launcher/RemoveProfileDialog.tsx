import { useState } from "react";
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
  if (profile && profile !== shown) setShown(profile);
  const current = profile ?? shown;
  return (
    <Modal
      open={profile !== null}
      onOpenChange={(o) => !o && onClose()}
      size="sm"
      closeOnBackdrop={false}
    >
      {current && (
        <Body
          key={current.id}
          profile={current}
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
  onClose,
  onRemoved,
  api,
}: Omit<Props, "profile"> & { profile: ProfileSummary }) {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const remove = () => {
    setBusy(true);
    setError(null);
    playSound("click");
    api.remove(profile.id).then(
      (snapshot) => onRemoved(snapshot),
      (e: unknown) => {
        setBusy(false);
        setError(describeError("profile", e));
      },
    );
  };

  return (
    <>
      <ModalHeader title={t("removeProfile.title", { profile: profile.name })} />
      <p className="text-graphite mt-4 text-[0.9375rem] leading-relaxed">
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
