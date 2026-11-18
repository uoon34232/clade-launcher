import { useMemo, useRef, useState } from "react";
import {
  formatGigabytes,
  formatPlaytime,
  MAX_NAME_CHARS,
  profileDetail,
  validateName,
} from "@/features/launcher/profileUi";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import {
  profilesApi,
  type ProfileSummary,
  type ProfilesApi,
  type ProfilesSnapshot,
} from "@/lib/profiles/api";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader, ModalSection } from "./Modal";
import { BUTTON_DANGER, BUTTON_SMALL_SOFT, BUTTON_SOFT, FIELD, LABEL } from "./modalStyles";
import { Segmented } from "./Segmented";

interface Props {
  /** The profile being edited; `null` keeps the dialog closed. */
  profile: ProfileSummary | null;
  /** All profiles in Clade, so duplicate checks exclude `profile.id` and allow case-only renames. */
  profiles: readonly ProfileSummary[];
  /** True while this profile's game is launching or running. */
  running?: boolean;
  verifyDisabled?: boolean;
  removeDisabled?: boolean;
  onClose: () => void;
  onUpdated: (snapshot: ProfilesSnapshot) => void;
  onVerify: () => void;
  onRemove: () => void;
  api?: ProfilesApi;
}

/**
 * Per-profile settings: display name, status and playtime, optional memory override, and direct
 * actions for the profile's own files and removal from Clade.
 */
export function ProfileDialog({
  profile,
  profiles,
  running = false,
  verifyDisabled = false,
  removeDisabled = false,
  onClose,
  onUpdated,
  onVerify,
  onRemove,
  api = profilesApi,
}: Props) {
  // Keep the last non-null profile while the closing animation runs.
  const [shown, setShown] = useState(profile);
  const [wasOpen, setWasOpen] = useState(profile !== null);
  const [session, setSession] = useState(0);
  const [saving, setSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  if (profile && profile !== shown) {
    if (shown && profile.name !== shown.name) setDirty(false);
    setShown(profile);
  }
  const isOpen = profile !== null;
  if (isOpen !== wasOpen) {
    setWasOpen(isOpen);
    if (isOpen) setSession((n) => n + 1);
    setSaving(false);
    setDirty(false);
  } else if ((saving || dirty) && profile && shown && profile.id !== shown.id) {
    setSaving(false);
    setDirty(false);
  }

  const current = profile ?? shown;

  return (
    <Modal
      open={isOpen}
      onOpenChange={(o) => !o && onClose()}
      size="md"
      locked={saving}
      closeOnBackdrop={!dirty}
    >
      {current && (
        <Body
          key={`${current.id}:${session}`}
          profile={current}
          profiles={profiles}
          running={running}
          verifyDisabled={verifyDisabled}
          removeDisabled={removeDisabled}
          saving={saving}
          setSaving={setSaving}
          setDirty={setDirty}
          onClose={onClose}
          onUpdated={onUpdated}
          onVerify={onVerify}
          onRemove={onRemove}
          api={api}
        />
      )}
    </Modal>
  );
}

interface BodyProps {
  profile: ProfileSummary;
  profiles: readonly ProfileSummary[];
  running: boolean;
  verifyDisabled: boolean;
  removeDisabled: boolean;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  setDirty: (dirty: boolean) => void;
  onClose: () => void;
  onUpdated: (snapshot: ProfilesSnapshot) => void;
  onVerify: () => void;
  onRemove: () => void;
  api: ProfilesApi;
}

function Body({
  profile,
  profiles,
  running,
  verifyDisabled,
  removeDisabled,
  saving,
  setSaving,
  setDirty,
  onClose,
  onUpdated,
  onVerify,
  onRemove,
  api,
}: BodyProps) {
  const t = useT();
  const ui = useUiSettings();
  const { min, max, auto } = ui.memory;
  const launcherDefaultMb = Math.min(max, Math.max(min, ui.memoryMb ?? auto));

  const otherNames = useMemo(
    () => profiles.filter((p) => p.id !== profile.id).map((p) => p.name),
    [profiles, profile.id],
  );

  const [prevProfileName, setPrevProfileName] = useState(profile.name);
  const [name, setName] = useState(profile.name);
  const [nameTouched, setNameTouched] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  if (profile.name !== prevProfileName) {
    setPrevProfileName(profile.name);
    setName(profile.name);
    setNameTouched(false);
  }

  const trimmed = name.trim();
  const unchanged = trimmed === profile.name;
  const nameError = validateName(name, otherNames);
  const showNameError = nameError !== null && (nameTouched || nameError !== "validate.nameEmpty");

  const saveRename = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    setNameTouched(true);
    if (unchanged || nameError !== null || running || saving) return;
    setSaving(true);
    setRenameError(null);
    api.rename(profile.id, trimmed).then(
      (snapshot) => {
        playSound("primary");
        setSaving(false);
        setDirty(false);
        setName(trimmed);
        setNameTouched(false);
        onUpdated(snapshot);
      },
      (err: unknown) => {
        setSaving(false);
        setRenameError(describeError("profile", err));
      },
    );
  };

  const savedMb = profile.memoryMb ?? null;
  const custom = savedMb !== null;
  const [draft, setDraft] = useState<number | null>(null);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const inFlightMb = useRef<number | null>(null);
  const value = Math.min(max, Math.max(min, draft ?? savedMb ?? launcherDefaultMb));

  const commitMemory = (rawMb: number) => {
    if (running || !Number.isFinite(rawMb)) return;
    const mb = Math.min(max, Math.max(min, rawMb));
    if (mb === savedMb || inFlightMb.current === mb) {
      if (mb === savedMb) setDraft(null);
      return;
    }
    inFlightMb.current = mb;
    setMemoryError(null);
    api.setMemory(profile.id, mb).then(
      (snap) => {
        if (inFlightMb.current === mb) inFlightMb.current = null;
        setDraft(null);
        onUpdated(snap);
      },
      (e: unknown) => {
        if (inFlightMb.current === mb) inFlightMb.current = null;
        setDraft(null);
        setMemoryError(describeError("profile", e));
      },
    );
  };

  const [filesError, setFilesError] = useState<string | null>(null);

  return (
    <>
      <div className="-mb-3">
        <ModalHeader title={profile.name} description={profileDetail(profile)} />
      </div>

      <ModalSection
        compact
        className="gap-1.5 border-t-0 pt-0 pb-2"
        title={t("profileModal.details")}
      >
        <form onSubmit={saveRename} className="flex flex-col gap-1" noValidate>
          <label htmlFor="profile-edit-name" className={LABEL}>
            {t("create.name")}
          </label>
          <div className="flex items-center gap-2">
            <input
              id="profile-edit-name"
              value={name}
              disabled={running || saving}
              onChange={(e) => {
                const next = e.target.value;
                setName(next);
                setDirty(next !== profile.name);
                setRenameError(null);
              }}
              onBlur={() => setNameTouched(true)}
              autoComplete="off"
              spellCheck={false}
              maxLength={80}
              aria-invalid={showNameError ? true : undefined}
              aria-describedby={showNameError ? "profile-edit-name-error" : undefined}
              className={`${FIELD} h-10 min-w-0 flex-1`}
            />
            <button
              type="submit"
              disabled={unchanged || nameError !== null || running || saving}
              className={`${BUTTON_SOFT} h-10 shrink-0 px-4`}
            >
              {saving ? t("profileModal.saving") : t("profileModal.saveName")}
            </button>
          </div>
          {showNameError && (
            <p
              id="profile-edit-name-error"
              role="alert"
              className="text-clay-deep text-[0.8125rem]"
            >
              {t(nameError, { max: MAX_NAME_CHARS })}
            </p>
          )}
          {renameError && !showNameError && (
            <p role="alert" className="text-clay-deep text-[0.8125rem]">
              {renameError}
            </p>
          )}
        </form>
        <dl className="flex flex-wrap gap-x-6 gap-y-1 text-[0.8125rem]">
          <div className="flex gap-2">
            <dt className="text-ink-muted">{t("profileModal.status")}</dt>
            <dd className="text-graphite font-medium">
              {profile.ready ? t("profileModal.statusReady") : t("profileModal.statusPending")}
            </dd>
          </div>
          <div className="flex gap-2">
            <dt className="text-ink-muted">{t("profileModal.playtime")}</dt>
            <dd className="text-graphite font-medium tabular-nums">
              {formatPlaytime(profile.playtimeSeconds)}
            </dd>
          </div>
        </dl>
      </ModalSection>

      <ModalSection compact className="gap-1.5 py-2" title={t("settings.memory")}>
        <Segmented<"default" | "custom">
          compact
          className="self-start"
          disabled={running}
          label={t("settings.memory")}
          value={custom ? "custom" : "default"}
          onChange={(mode) => {
            if ((mode === "custom") === custom) return;
            playSound("select");
            inFlightMb.current = null;
            setDraft(null);
            setMemoryError(null);
            const nextMb = mode === "default" ? null : launcherDefaultMb;
            api.setMemory(profile.id, nextMb).then(onUpdated, (e: unknown) => {
              setMemoryError(describeError("profile", e));
            });
          }}
          options={[
            { value: "default", label: t("profileModal.memoryDefault") },
            { value: "custom", label: t("settings.memory.custom") },
          ]}
        />
        {custom ? (
          <div className={`flex items-center gap-3 ${running ? "opacity-45" : ""}`}>
            <input
              id="profile-memory-slider"
              type="range"
              min={min}
              max={max}
              step={256}
              disabled={running}
              value={value}
              onChange={(e) => {
                setDraft(Number(e.target.value));
                setMemoryError(null);
              }}
              onPointerUp={(e) => commitMemory(Number(e.currentTarget.value))}
              onKeyUp={(e) => commitMemory(Number(e.currentTarget.value))}
              onBlur={(e) => commitMemory(Number(e.currentTarget.value))}
              aria-label={t("settings.memory.slider")}
              aria-valuetext={formatGigabytes(value)}
              className="accent-clay focus-visible:ring-clay/45 h-6 min-w-0 flex-1 cursor-default outline-none focus-visible:ring-2 disabled:cursor-not-allowed"
            />
            <span
              aria-hidden
              className="text-graphite w-14 text-right text-[0.875rem] tabular-nums"
            >
              {formatGigabytes(value)}
            </span>
          </div>
        ) : (
          <p className="text-ink-muted text-[0.8125rem]">
            {t("profileModal.memoryDefaultNote", { value: formatGigabytes(launcherDefaultMb) })}
          </p>
        )}
        {memoryError && (
          <p role="alert" className="text-clay-deep text-[0.8125rem]">
            {memoryError}
          </p>
        )}
      </ModalSection>

      <ModalSection compact className="gap-1.5 py-2" title={t("profileModal.files")}>
        <p className="text-ink-muted text-[0.8125rem]">
          {profile.kind === "modded"
            ? t("profileModal.filesNoteModded")
            : t("profileModal.filesNoteVanilla")}
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            className={BUTTON_SMALL_SOFT}
            onClick={() => {
              playSound("click");
              setFilesError(null);
              api.openFolder(profile.id).catch((e: unknown) => {
                setFilesError(describeError("profile", e));
              });
            }}
          >
            {t("profile.openFolder")}
          </button>
          <button
            type="button"
            disabled={verifyDisabled || saving}
            className={BUTTON_SMALL_SOFT}
            onClick={() => {
              playSound("click");
              onVerify();
            }}
          >
            {t("profile.verify")}
          </button>
        </div>
        {filesError && (
          <p role="alert" className="text-clay-deep text-[0.8125rem]">
            {filesError}
          </p>
        )}
      </ModalSection>

      <ModalFooter>
        <button
          type="button"
          disabled={removeDisabled || saving}
          className={`${BUTTON_DANGER} mr-auto`}
          onClick={() => {
            playSound("click");
            onRemove();
          }}
        >
          {t("profile.remove")}
        </button>
        <button
          type="button"
          disabled={saving}
          className={BUTTON_SOFT}
          onClick={() => {
            playSound("click");
            onClose();
          }}
        >
          {t("common.done")}
        </button>
      </ModalFooter>
    </>
  );
}
