import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  defaultVersion,
  FILTER_KEYS,
  filterVersions,
  formatGigabytes,
  formatPlaytime,
  MAX_NAME_CHARS,
  profileDetail,
  releaseDay,
  staleNote,
  validateName,
  type VersionFilter,
} from "@/features/launcher/profileUi";
import { useT, type MessageKey } from "@/lib/i18n";
import { describeCode, describeError } from "@/lib/i18n/errors";
import {
  LOADER_NAMES,
  MAX_RESOLUTION_HEIGHT,
  MAX_RESOLUTION_WIDTH,
  MIN_RESOLUTION_HEIGHT,
  MIN_RESOLUTION_WIDTH,
  profilesApi,
  type LoaderOption,
  type ProfileSummary,
  type ProfilesApi,
  type ProfilesSnapshot,
  type VersionCatalog,
} from "@/lib/profiles/api";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound } from "@/lib/sound/soundService";
import { cn } from "@/lib/utils";
import { Modal, ModalFooter, ModalHeader, ModalSection } from "./Modal";
import {
  BUTTON_DANGER,
  BUTTON_QUIET,
  BUTTON_SMALL_PRIMARY,
  BUTTON_SMALL_QUIET,
  BUTTON_SMALL_SOFT,
  BUTTON_SOFT,
  FIELD,
  FIELD_COMPACT,
  LABEL,
} from "./modalStyles";
import { Segmented } from "./Segmented";

const NON_VALUE_KEYS = new Set(["Tab", "Escape", "Shift", "Control", "Alt", "Meta"]);

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
  onDuplicated?: (snapshot: ProfilesSnapshot) => void;
  onVerify: () => void;
  onRemove: () => void;
  api?: ProfilesApi;
}

type CatalogState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; data: VersionCatalog };

const CHANNEL_NOTE: Record<string, MessageKey> = {
  recommended: "create.channel.recommended",
  stable: "create.channel.stable",
  latest: "create.channel.latest",
  prerelease: "create.channel.prerelease",
};

function filterForKind(kind: VersionCatalog["entries"][number]["kind"] | undefined): VersionFilter {
  if (kind === "snapshot") return "snapshots";
  if (kind === "old_beta" || kind === "old_alpha") return "historical";
  return "releases";
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
  onDuplicated,
  onVerify,
  onRemove,
  api = profilesApi,
}: Props) {
  // Keep the last non-null profile while the closing animation runs.
  const [shown, setShown] = useState(profile);
  const [wasOpen, setWasOpen] = useState(profile !== null);
  const [session, setSession] = useState(0);
  const [saving, setSaving] = useState(false);
  const [auxSaving, setAuxSaving] = useState(false);
  const [dirty, setDirty] = useState(false);

  if (profile && profile !== shown) {
    setShown(profile);
  }
  const isOpen = profile !== null;
  if (isOpen !== wasOpen) {
    setWasOpen(isOpen);
    if (isOpen) setSession((n) => n + 1);
    setSaving(false);
    setAuxSaving(false);
    setDirty(false);
  } else if ((saving || auxSaving || dirty) && profile && shown && profile.id !== shown.id) {
    setSaving(false);
    setAuxSaving(false);
    setDirty(false);
  }

  const ui = useUiSettings();
  const isAdvanced = ui.uiMode === "advanced";
  const current = profile ?? shown;

  return (
    <Modal
      open={isOpen}
      onOpenChange={(o) => !o && onClose()}
      size={isAdvanced ? "xl" : "md"}
      locked={saving || auxSaving}
      closeOnBackdrop={!dirty}
    >
      {current && (
        <Body
          key={`${current.id}:${session}`}
          active={isOpen}
          profile={current}
          profiles={profiles}
          running={running}
          verifyDisabled={verifyDisabled}
          removeDisabled={removeDisabled}
          saving={saving}
          setSaving={setSaving}
          setAuxSaving={setAuxSaving}
          setDirty={setDirty}
          onClose={onClose}
          onUpdated={onUpdated}
          onDuplicated={onDuplicated ?? onUpdated}
          onVerify={onVerify}
          onRemove={onRemove}
          api={api}
        />
      )}
    </Modal>
  );
}

interface BodyProps {
  active: boolean;
  profile: ProfileSummary;
  profiles: readonly ProfileSummary[];
  running: boolean;
  verifyDisabled: boolean;
  removeDisabled: boolean;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  setAuxSaving: (auxSaving: boolean) => void;
  setDirty: (dirty: boolean) => void;
  onClose: () => void;
  onUpdated: (snapshot: ProfilesSnapshot) => void;
  onDuplicated: (snapshot: ProfilesSnapshot) => void;
  onVerify: () => void;
  onRemove: () => void;
  api: ProfilesApi;
}

function Body({
  active,
  profile,
  profiles,
  running,
  verifyDisabled,
  removeDisabled,
  saving,
  setSaving,
  setAuxSaving,
  setDirty,
  onClose,
  onUpdated,
  onDuplicated,
  onVerify,
  onRemove,
  api,
}: BodyProps) {
  const t = useT();
  const ui = useUiSettings();
  const isAdvanced = ui.uiMode === "advanced";
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

  const savedMb = profile.memoryMb ?? null;
  const custom = savedMb !== null;
  const [draft, setDraft] = useState<number | null>(null);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const inFlightMemoryRef = useRef<number | null | undefined>(undefined);
  const value = Math.min(max, Math.max(min, draft ?? savedMb ?? launcherDefaultMb));

  const mountedRef = useRef(true);
  const activeRef = useRef(active);
  const renameSeqRef = useRef(0);
  const memorySeqRef = useRef(0);
  const memoryDragSeqRef = useRef(0);
  const resolutionSeqRef = useRef(0);
  const jvmSeqRef = useRef(0);
  const duplicateSeqRef = useRef(0);
  const openFolderSeqRef = useRef(0);
  const openFolderInFlightRef = useRef(false);
  const restoreVersionFocusRef = useRef(false);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const pendingRenameFocusRef = useRef(false);
  const resolutionWrapRef = useRef<HTMLDivElement>(null);
  const pendingResFocusRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      setAuxSaving(false);
    };
  }, [setAuxSaving]);

  useLayoutEffect(() => {
    activeRef.current = active;
    if (!active) {
      inFlightMemoryRef.current = undefined;
      openFolderInFlightRef.current = false;
      restoreVersionFocusRef.current = false;
      pendingRenameFocusRef.current = false;
      pendingResFocusRef.current = false;
      renameSeqRef.current += 1;
      memorySeqRef.current += 1;
      memoryDragSeqRef.current += 1;
      resolutionSeqRef.current += 1;
      jvmSeqRef.current += 1;
      duplicateSeqRef.current += 1;
      openFolderSeqRef.current += 1;
    }
  }, [active]);

  const savedRes = profile.resolution ?? null;
  const nextResKey = savedRes ? `${savedRes.width}x${savedRes.height}` : "";
  const [prevResKey, setPrevResKey] = useState(nextResKey);
  const [widthText, setWidthText] = useState(savedRes ? String(savedRes.width) : "1280");
  const [heightText, setHeightText] = useState(savedRes ? String(savedRes.height) : "720");
  const [resolutionError, setResolutionError] = useState<string | null>(null);
  const [resolutionSaving, setResolutionSaving] = useState(false);
  if (nextResKey !== prevResKey) {
    setPrevResKey(nextResKey);
    setWidthText(savedRes ? String(savedRes.width) : "1280");
    setHeightText(savedRes ? String(savedRes.height) : "720");
    setResolutionError(null);
  }
  const parseDim = (s: string) => {
    const t = s.trim();
    return /^\d+$/.test(t) ? Number(t) : null;
  };
  const resolutionUnchanged =
    savedRes === null ||
    (parseDim(widthText) === savedRes.width && parseDim(heightText) === savedRes.height);

  const savedJvmArgs = useMemo(() => profile.jvmArgs ?? [], [profile.jvmArgs]);
  const savedJvmJoined = savedJvmArgs.join(" ");
  const [prevJvmJoined, setPrevJvmJoined] = useState(savedJvmJoined);
  const [jvmText, setJvmText] = useState(savedJvmJoined);
  const [jvmError, setJvmError] = useState<string | null>(null);
  const [jvmSaving, setJvmSaving] = useState(false);
  if (savedJvmJoined !== prevJvmJoined) {
    setPrevJvmJoined(savedJvmJoined);
    setJvmText(savedJvmJoined);
    setJvmError(null);
  }
  const nextJvmArgs = useMemo(() => (jvmText.trim() ? jvmText.trim().split(/\s+/) : []), [jvmText]);
  const jvmUnchanged =
    nextJvmArgs.length === savedJvmArgs.length &&
    nextJvmArgs.every((arg, i) => arg === savedJvmArgs[i]);

  useEffect(() => {
    setAuxSaving(resolutionSaving || jvmSaving);
  }, [resolutionSaving, jvmSaving, setAuxSaving]);

  const [changingVersion, setChangingVersion] = useState(false);
  const changeVersionBtnRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!changingVersion && restoreVersionFocusRef.current) {
      restoreVersionFocusRef.current = false;
      changeVersionBtnRef.current?.focus();
    }
  }, [changingVersion]);
  const [renaming, setRenaming] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [filesError, setFilesError] = useState<string | null>(null);
  const anySaving = saving || resolutionSaving || jvmSaving;

  useEffect(() => {
    if (renaming || !pendingRenameFocusRef.current) return;
    pendingRenameFocusRef.current = false;
    const activeEl = document.activeElement;
    if (!activeEl || activeEl === document.body || activeEl === document.documentElement) {
      renameInputRef.current?.focus();
    }
  }, [renaming]);

  useEffect(() => {
    if (resolutionSaving || !pendingResFocusRef.current) return;
    pendingResFocusRef.current = false;
    const activeEl = document.activeElement;
    if (!activeEl || activeEl === document.body || activeEl === document.documentElement) {
      resolutionWrapRef.current
        ?.querySelector<HTMLElement>('[role="radio"][aria-checked="true"], [role="radio"]')
        ?.focus();
    }
  }, [resolutionSaving]);

  const computeDirty = (over?: {
    baselineName?: string;
    name?: string;
    changingVersion?: boolean;
    resDirty?: boolean;
    jvmDirty?: boolean;
  }) =>
    (over?.name ?? name) !== (over?.baselineName ?? profile.name) ||
    (over?.changingVersion ?? changingVersion) ||
    (over?.resDirty ?? !resolutionUnchanged) ||
    (over?.jvmDirty ?? !jvmUnchanged);

  const saveRename = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    setNameTouched(true);
    if (unchanged || nameError !== null || running || anySaving) return;
    const seq = ++renameSeqRef.current;
    pendingRenameFocusRef.current = true;
    setRenaming(true);
    setSaving(true);
    setRenameError(null);
    api.rename(profile.id, trimmed).then(
      (snapshot) => {
        if (!mountedRef.current || !activeRef.current || renameSeqRef.current !== seq) return;
        playSound("primary");
        setRenaming(false);
        setSaving(false);
        setDirty(computeDirty({ baselineName: trimmed, name: trimmed }));
        setName(trimmed);
        setNameTouched(false);
        onUpdated(snapshot);
      },
      (err: unknown) => {
        if (!mountedRef.current || !activeRef.current || renameSeqRef.current !== seq) return;
        setRenaming(false);
        setSaving(false);
        setRenameError(describeError("profile", err));
      },
    );
  };

  const commitMemory = (rawMb: number) => {
    if (running || anySaving || !Number.isFinite(rawMb)) return;
    const mb = Math.min(max, Math.max(min, rawMb));
    if (
      (mb === savedMb && inFlightMemoryRef.current === undefined) ||
      inFlightMemoryRef.current === mb
    ) {
      if (mb === savedMb && inFlightMemoryRef.current === undefined) setDraft(null);
      return;
    }
    const seq = ++memorySeqRef.current;
    const dragSeq = memoryDragSeqRef.current;
    inFlightMemoryRef.current = mb;
    setMemoryError(null);
    api.setMemory(profile.id, mb).then(
      (snap) => {
        if (!mountedRef.current || !activeRef.current || memorySeqRef.current !== seq) return;
        inFlightMemoryRef.current = undefined;
        if (memoryDragSeqRef.current === dragSeq) setDraft(null);
        onUpdated(snap);
      },
      (e: unknown) => {
        if (!mountedRef.current || !activeRef.current || memorySeqRef.current !== seq) return;
        inFlightMemoryRef.current = undefined;
        if (memoryDragSeqRef.current === dragSeq) setDraft(null);
        setMemoryError(describeError("profile", e));
      },
    );
  };

  const saveResolution = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    if (running || anySaving || savedRes === null) return;
    const wRaw = widthText.trim();
    const hRaw = heightText.trim();
    const width = /^\d+$/.test(wRaw) ? Number(wRaw) : Number.NaN;
    const height = /^\d+$/.test(hRaw) ? Number(hRaw) : Number.NaN;
    if (
      !Number.isInteger(width) ||
      !Number.isInteger(height) ||
      width < MIN_RESOLUTION_WIDTH ||
      width > MAX_RESOLUTION_WIDTH ||
      height < MIN_RESOLUTION_HEIGHT ||
      height > MAX_RESOLUTION_HEIGHT
    ) {
      setResolutionError(
        describeCode("profile", "invalid_resolution", {
          minWidth: MIN_RESOLUTION_WIDTH,
          maxWidth: MAX_RESOLUTION_WIDTH,
          minHeight: MIN_RESOLUTION_HEIGHT,
          maxHeight: MAX_RESOLUTION_HEIGHT,
        }),
      );
      return;
    }
    const seq = ++resolutionSeqRef.current;
    setResolutionSaving(true);
    setResolutionError(null);
    api.setResolution(profile.id, { width, height }).then(
      (snap) => {
        if (!mountedRef.current || !activeRef.current || resolutionSeqRef.current !== seq) return;
        playSound("primary");
        setResolutionSaving(false);
        setWidthText(String(width));
        setHeightText(String(height));
        setDirty(computeDirty({ resDirty: false }));
        onUpdated(snap);
      },
      (err: unknown) => {
        if (!mountedRef.current || !activeRef.current || resolutionSeqRef.current !== seq) return;
        setResolutionSaving(false);
        setResolutionError(describeError("profile", err));
      },
    );
  };

  const saveJvmArgs = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    if (running || anySaving || jvmUnchanged) return;
    const seq = ++jvmSeqRef.current;
    setJvmSaving(true);
    setJvmError(null);
    api.setJvmArgs(profile.id, nextJvmArgs).then(
      (snap) => {
        if (!mountedRef.current || !activeRef.current || jvmSeqRef.current !== seq) return;
        playSound("primary");
        setJvmSaving(false);
        setDirty(computeDirty({ jvmDirty: false }));
        onUpdated(snap);
      },
      (err: unknown) => {
        if (!mountedRef.current || !activeRef.current || jvmSeqRef.current !== seq) return;
        setJvmSaving(false);
        setJvmError(describeError("profile", err));
      },
    );
  };

  const duplicateProfile = () => {
    if (running || anySaving) return;
    playSound("click");
    const seq = ++duplicateSeqRef.current;
    setSaving(true);
    setDuplicating(true);
    setFilesError(null);
    api.duplicate(profile.id).then(
      (snapshot) => {
        if (!mountedRef.current || !activeRef.current || duplicateSeqRef.current !== seq) return;
        playSound("primary");
        setSaving(false);
        setDuplicating(false);
        onDuplicated(snapshot);
      },
      (err: unknown) => {
        if (!mountedRef.current || !activeRef.current || duplicateSeqRef.current !== seq) return;
        setSaving(false);
        setDuplicating(false);
        setFilesError(describeError("profile", err));
      },
    );
  };

  const detailsSection = (
    <ModalSection
      compact
      className="gap-1 border-t-0 pt-0 pb-1.5"
      title={t("profileModal.details")}
    >
      <form onSubmit={saveRename} className="flex flex-col gap-1" noValidate>
        <label htmlFor="profile-edit-name" className={LABEL}>
          {t("create.name")}
        </label>
        <div className="flex items-center gap-2">
          <input
            ref={renameInputRef}
            id="profile-edit-name"
            value={name}
            disabled={running || anySaving}
            onChange={(e) => {
              const next = e.target.value;
              setName(next);
              setDirty(computeDirty({ name: next }));
              setRenameError(null);
            }}
            onBlur={() => setNameTouched(true)}
            autoComplete="off"
            spellCheck={false}
            maxLength={80}
            aria-invalid={showNameError || renameError ? true : undefined}
            aria-describedby={
              showNameError
                ? "profile-edit-name-error"
                : renameError
                  ? "profile-edit-name-server-error"
                  : undefined
            }
            className={cn(FIELD, "h-10 min-w-0 flex-1")}
          />
          <button
            type="submit"
            disabled={unchanged || nameError !== null || running || anySaving}
            className={cn(BUTTON_SOFT, "h-10 shrink-0 px-4")}
          >
            {renaming ? t("profileModal.saving") : t("profileModal.saveName")}
          </button>
        </div>
        {showNameError && (
          <p id="profile-edit-name-error" role="alert" className="text-clay-deep text-[0.8125rem]">
            {t(nameError, { max: MAX_NAME_CHARS })}
          </p>
        )}
        {renameError && !showNameError && (
          <p
            id="profile-edit-name-server-error"
            role="alert"
            className="text-clay-deep text-[0.8125rem]"
          >
            {renameError}
          </p>
        )}
      </form>
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1">
        <dl className="flex flex-wrap gap-x-4 gap-y-1 text-[0.8125rem]">
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
        {!changingVersion && (
          <button
            ref={changeVersionBtnRef}
            type="button"
            disabled={running || anySaving}
            className={BUTTON_SMALL_SOFT}
            onClick={() => {
              playSound("click");
              setChangingVersion(true);
              setDirty(true);
            }}
          >
            {t("profileModal.changeVersion")}
          </button>
        )}
      </div>
      {changingVersion && (
        <VersionEditor
          active={active}
          profile={profile}
          isAdvanced={isAdvanced}
          running={running}
          saving={anySaving}
          setSaving={setSaving}
          api={api}
          onCancel={() => {
            playSound("click");
            restoreVersionFocusRef.current = true;
            setChangingVersion(false);
            setDirty(computeDirty({ changingVersion: false }));
          }}
          onSaved={(snapshot) => {
            restoreVersionFocusRef.current = true;
            setChangingVersion(false);
            setDirty(computeDirty({ changingVersion: false }));
            onUpdated(snapshot);
          }}
        />
      )}
    </ModalSection>
  );

  const filesSection = (
    <ModalSection compact className="gap-1 py-1.5" title={t("profileModal.files")}>
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
            if (openFolderInFlightRef.current) return;
            openFolderInFlightRef.current = true;
            playSound("click");
            const seq = ++openFolderSeqRef.current;
            setFilesError(null);
            api.openFolder(profile.id).then(
              () => {
                if (openFolderSeqRef.current === seq) openFolderInFlightRef.current = false;
              },
              (e: unknown) => {
                if (openFolderSeqRef.current === seq) openFolderInFlightRef.current = false;
                if (!mountedRef.current || !activeRef.current || openFolderSeqRef.current !== seq)
                  return;
                setFilesError(describeError("profile", e));
              },
            );
          }}
        >
          {t("profile.openFolder")}
        </button>
        <button
          type="button"
          disabled={verifyDisabled || anySaving}
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
  );

  const memorySection = (
    <ModalSection
      compact
      className="gap-1.5 border-t py-2 sm:border-t-0 sm:pt-0"
      title={t("settings.memory")}
    >
      <Segmented<"default" | "custom">
        compact
        className="self-start"
        disabled={running || anySaving}
        label={t("settings.memory")}
        value={custom ? "custom" : "default"}
        onChange={(mode) => {
          const nextMb = mode === "default" ? null : launcherDefaultMb;
          if (
            (mode === "custom") === custom ||
            running ||
            anySaving ||
            inFlightMemoryRef.current === nextMb
          ) {
            return;
          }
          playSound("select");
          memoryDragSeqRef.current += 1;
          const seq = ++memorySeqRef.current;
          inFlightMemoryRef.current = nextMb;
          setDraft(null);
          setMemoryError(null);
          api.setMemory(profile.id, nextMb).then(
            (snap) => {
              if (!mountedRef.current || !activeRef.current || memorySeqRef.current !== seq) return;
              inFlightMemoryRef.current = undefined;
              onUpdated(snap);
            },
            (e: unknown) => {
              if (!mountedRef.current || !activeRef.current || memorySeqRef.current !== seq) return;
              inFlightMemoryRef.current = undefined;
              setMemoryError(describeError("profile", e));
            },
          );
        }}
        options={[
          { value: "default", label: t("profileModal.memoryDefault") },
          { value: "custom", label: t("settings.memory.custom") },
        ]}
      />
      {custom ? (
        <div className="flex items-center gap-3">
          <input
            id="profile-memory-slider"
            type="range"
            min={min}
            max={max}
            step={256}
            disabled={running || anySaving}
            value={value}
            onChange={(e) => {
              memoryDragSeqRef.current += 1;
              setDraft(Number(e.target.value));
              setMemoryError(null);
            }}
            onPointerUp={(e) => commitMemory(Number(e.currentTarget.value))}
            onKeyUp={(e) => {
              if (!NON_VALUE_KEYS.has(e.key)) commitMemory(Number(e.currentTarget.value));
            }}
            onBlur={(e) => commitMemory(Number(e.currentTarget.value))}
            aria-label={t("settings.memory.slider")}
            aria-valuetext={formatGigabytes(value)}
            aria-invalid={memoryError ? true : undefined}
            aria-describedby={memoryError ? "profile-memory-error" : undefined}
            className="accent-clay focus-visible:ring-ring h-6 min-w-0 flex-1 cursor-default outline-hidden focus-visible:ring-2 disabled:cursor-not-allowed"
          />
          <span
            aria-hidden
            className={`w-14 shrink-0 text-right text-[0.875rem] whitespace-nowrap tabular-nums ${
              running || anySaving
                ? "text-ink-muted opacity-70 forced-colors:text-[GrayText] forced-colors:opacity-100"
                : "text-graphite"
            }`}
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
        <p id="profile-memory-error" role="alert" className="text-clay-deep text-[0.8125rem]">
          {memoryError}
        </p>
      )}
    </ModalSection>
  );

  const resolutionSection = (
    <ModalSection compact className="gap-1.5 py-2" title={t("profileModal.resolution")}>
      <div ref={resolutionWrapRef} className="self-start">
        <Segmented<"default" | "fixed">
          compact
          disabled={running || anySaving}
          label={t("profileModal.resolution")}
          value={savedRes !== null ? "fixed" : "default"}
          onChange={(mode) => {
            if ((mode === "fixed") === (savedRes !== null) || running || anySaving) return;
            playSound("select");
            pendingResFocusRef.current = true;
            const seq = ++resolutionSeqRef.current;
            setResolutionError(null);
            setResolutionSaving(true);
            const nextRes = mode === "default" ? null : { width: 1280, height: 720 };
            api.setResolution(profile.id, nextRes).then(
              (snap) => {
                if (!mountedRef.current || !activeRef.current || resolutionSeqRef.current !== seq)
                  return;
                setResolutionSaving(false);
                setDirty(computeDirty({ resDirty: false }));
                onUpdated(snap);
              },
              (e: unknown) => {
                if (!mountedRef.current || !activeRef.current || resolutionSeqRef.current !== seq)
                  return;
                setResolutionSaving(false);
                setResolutionError(describeError("profile", e));
              },
            );
          }}
          options={[
            { value: "default", label: t("profileModal.resolutionDefault") },
            { value: "fixed", label: t("profileModal.resolutionFixed") },
          ]}
        />
      </div>
      {savedRes !== null ? (
        <form onSubmit={saveResolution} className="flex flex-wrap items-center gap-1.5" noValidate>
          <input
            type="text"
            inputMode="numeric"
            maxLength={4}
            value={widthText}
            disabled={running || anySaving}
            onChange={(e) => {
              const nextW = e.target.value;
              setWidthText(nextW);
              setResolutionError(null);
              const resDirty =
                parseDim(nextW) !== savedRes.width || parseDim(heightText) !== savedRes.height;
              setDirty(computeDirty({ resDirty }));
            }}
            aria-label={t("profileModal.resolutionWidth")}
            aria-invalid={resolutionError ? true : undefined}
            aria-describedby={resolutionError ? "profile-resolution-error" : undefined}
            className={cn(FIELD_COMPACT, "w-16 shrink-0 text-center tabular-nums")}
          />
          <span aria-hidden className="text-ink-muted text-[0.875rem]">
            ×
          </span>
          <input
            type="text"
            inputMode="numeric"
            maxLength={4}
            value={heightText}
            disabled={running || anySaving}
            onChange={(e) => {
              const nextH = e.target.value;
              setHeightText(nextH);
              setResolutionError(null);
              const resDirty =
                parseDim(widthText) !== savedRes.width || parseDim(nextH) !== savedRes.height;
              setDirty(computeDirty({ resDirty }));
            }}
            aria-label={t("profileModal.resolutionHeight")}
            aria-invalid={resolutionError ? true : undefined}
            aria-describedby={resolutionError ? "profile-resolution-error" : undefined}
            className={cn(FIELD_COMPACT, "w-16 shrink-0 text-center tabular-nums")}
          />
          <button
            type="submit"
            disabled={running || anySaving || resolutionUnchanged}
            aria-label={t("profileModal.applyResolution")}
            className={cn(BUTTON_SMALL_SOFT, "ml-auto h-9 shrink-0 px-3")}
          >
            {t("common.apply")}
          </button>
        </form>
      ) : (
        <p className="text-ink-muted text-[0.8125rem]">{t("profileModal.resolutionDefaultNote")}</p>
      )}
      {resolutionError && (
        <p id="profile-resolution-error" role="alert" className="text-clay-deep text-[0.8125rem]">
          {resolutionError}
        </p>
      )}
    </ModalSection>
  );

  const jvmArgsSection = (
    <ModalSection compact className="gap-1.5 py-2" title={t("profileModal.jvmArgs")}>
      <form onSubmit={saveJvmArgs} className="flex flex-col gap-1" noValidate>
        <div className="flex items-center gap-2">
          <input
            id="profile-jvm-args"
            value={jvmText}
            disabled={running || anySaving}
            onChange={(e) => {
              const nextText = e.target.value;
              setJvmText(nextText);
              setJvmError(null);
              const nextTokens = nextText.trim() ? nextText.trim().split(/\s+/) : [];
              const jvmDirty =
                nextTokens.length !== savedJvmArgs.length ||
                nextTokens.some((arg, i) => arg !== savedJvmArgs[i]);
              setDirty(computeDirty({ jvmDirty }));
            }}
            autoComplete="off"
            spellCheck={false}
            placeholder={t("profileModal.jvmArgsPlaceholder")}
            aria-label={t("profileModal.jvmArgs")}
            aria-invalid={jvmError ? true : undefined}
            aria-describedby={jvmError ? "profile-jvm-error" : "profile-jvm-note"}
            className={cn(FIELD, "h-9 min-w-0 flex-1 text-[0.8125rem]")}
          />
          <button
            type="submit"
            disabled={running || anySaving || jvmUnchanged}
            aria-label={t("profileModal.applyJvmArgs")}
            className={cn(BUTTON_SMALL_SOFT, "h-9 shrink-0 px-3")}
          >
            {t("common.apply")}
          </button>
        </div>
        {jvmError ? (
          <p id="profile-jvm-error" role="alert" className="text-clay-deep text-[0.8125rem]">
            {jvmError}
          </p>
        ) : (
          <p id="profile-jvm-note" className="text-ink-muted text-[0.8125rem]">
            {t("profileModal.jvmArgsNote")}
          </p>
        )}
      </form>
    </ModalSection>
  );

  return (
    <>
      <div className="-mb-4">
        <ModalHeader title={profile.name} description={profileDetail(profile)} />
      </div>

      {isAdvanced ? (
        <div className="grid gap-x-6 sm:grid-cols-2 lg:gap-x-8">
          <div className="flex min-w-0 flex-col">
            {detailsSection}
            {filesSection}
          </div>
          <div className="flex min-w-0 flex-col">
            {memorySection}
            {resolutionSection}
            {jvmArgsSection}
          </div>
        </div>
      ) : (
        <div className="flex min-w-0 flex-col">
          {detailsSection}
          {filesSection}
        </div>
      )}

      <ModalFooter>
        <button
          type="button"
          disabled={removeDisabled || anySaving}
          className={cn(BUTTON_DANGER, "mr-auto")}
          onClick={() => {
            playSound("click");
            onRemove();
          }}
        >
          {t("profile.remove")}
        </button>
        <button
          type="button"
          disabled={running || anySaving}
          className={BUTTON_QUIET}
          onClick={duplicateProfile}
        >
          {duplicating ? t("profileModal.duplicating") : t("profileModal.duplicate")}
        </button>
        <button
          type="button"
          disabled={anySaving}
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

interface VersionEditorProps {
  active: boolean;
  profile: ProfileSummary;
  isAdvanced: boolean;
  running: boolean;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  api: ProfilesApi;
  onCancel: () => void;
  onSaved: (snapshot: ProfilesSnapshot) => void;
}

function VersionEditor({
  active,
  profile,
  isAdvanced,
  running,
  saving,
  setSaving,
  api,
  onCancel,
  onSaved,
}: VersionEditorProps) {
  const t = useT();
  const editorRef = useRef<HTMLDivElement>(null);
  const mcTriggerRef = useRef<HTMLButtonElement>(null);
  const pendingCatalogFocusRef = useRef(false);
  const mountedRef = useRef(true);
  const activeRef = useRef(active);
  const saveSeqRef = useRef(0);
  useEffect(() => {
    editorRef.current?.focus();
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  useLayoutEffect(() => {
    activeRef.current = active;
    if (!active) saveSeqRef.current += 1;
  }, [active]);
  const [reload, setReload] = useState(0);
  const [filter, setFilter] = useState<VersionFilter>("releases");
  const [catalogResult, setCatalogResult] = useState<{
    key: number;
    state: Exclude<CatalogState, { status: "loading" }>;
  } | null>(null);
  const [mc, setMc] = useState<string>(profile.minecraft);
  const [loaderVersion, setLoaderVersion] = useState<string | null>(profile.loaderVersion);
  const [loaderResult, setLoaderResult] = useState<{
    mc: string;
    options: LoaderOption[] | null;
  } | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const catalog = useMemo<CatalogState>(
    () => (catalogResult?.key === reload ? catalogResult.state : { status: "loading" }),
    [catalogResult, reload],
  );

  useEffect(() => {
    if (!pendingCatalogFocusRef.current || catalog.status === "loading") return;
    pendingCatalogFocusRef.current = false;
    const activeEl = document.activeElement;
    if (activeEl && activeEl !== document.body && activeEl !== document.documentElement) return;
    if (catalog.status === "ready") {
      mcTriggerRef.current?.focus();
    } else {
      editorRef.current?.focus();
    }
  }, [catalog.status]);

  useEffect(() => {
    let cancelled = false;
    api.versionCatalog().then(
      (data) => {
        if (cancelled) return;
        const match = data.entries.find((e) => e.id === profile.minecraft);
        const initialFilter = isAdvanced ? filterForKind(match?.kind) : "releases";
        setFilter(initialFilter);
        setCatalogResult({ key: reload, state: { status: "ready", data } });
        const list = filterVersions(data.entries, initialFilter);
        setMc(
          list.some((e) => e.id === profile.minecraft)
            ? profile.minecraft
            : (defaultVersion(data, list, initialFilter) ?? profile.minecraft),
        );
      },
      (e: unknown) => {
        if (!cancelled) {
          setCatalogResult({
            key: reload,
            state: { status: "error", message: describeError("catalog", e) },
          });
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [reload, api, profile.minecraft, isAdvanced]);

  const activeFilter: VersionFilter = isAdvanced ? filter : "releases";
  const entries = useMemo(
    () => (catalog.status === "ready" ? catalog.data.entries : []),
    [catalog],
  );
  const shown = useMemo(() => filterVersions(entries, activeFilter), [entries, activeFilter]);
  const selectedMc =
    catalog.status === "ready"
      ? shown.some((e) => e.id === mc)
        ? mc
        : defaultVersion(catalog.data, shown, activeFilter)
      : mc;

  const moddedLoader = profile.kind === "modded" ? profile.loader : null;

  useEffect(() => {
    if (!moddedLoader || !selectedMc) return;
    let cancelled = false;
    api.loaderOptions(selectedMc).then(
      (options) => {
        if (!cancelled) setLoaderResult({ mc: selectedMc, options });
      },
      () => {
        if (!cancelled) setLoaderResult({ mc: selectedMc, options: null });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [moddedLoader, selectedMc, api]);

  const loaderForMc = loaderResult?.mc === selectedMc ? loaderResult : null;
  const loaderFailed = loaderForMc !== null && loaderForMc.options === null;
  const loadingLoaders = moddedLoader !== null && !!selectedMc && loaderForMc === null;
  const currentLoader = moddedLoader
    ? loaderForMc?.options?.find((o) => o.loader === moddedLoader)
    : undefined;
  const chosenLoaderVersion = currentLoader
    ? isAdvanced && loaderVersion && currentLoader.versions.some((v) => v.version === loaderVersion)
      ? loaderVersion
      : currentLoader.recommended
    : null;

  const isUnchanged =
    selectedMc === profile.minecraft &&
    (moddedLoader === null ||
      (chosenLoaderVersion !== null && chosenLoaderVersion === profile.loaderVersion));

  const canSave =
    !running &&
    !saving &&
    catalog.status === "ready" &&
    shown.length > 0 &&
    !!selectedMc &&
    !isUnchanged &&
    (moddedLoader === null ||
      (!loadingLoaders && currentLoader?.status === "ok" && !!chosenLoaderVersion));

  const changeFilter = (nextFilter: VersionFilter) => {
    setFilter(nextFilter);
    setSubmitError(null);
    if (catalog.status !== "ready") return;
    const next = filterVersions(catalog.data.entries, nextFilter);
    if (!next.some((e) => e.id === mc)) {
      const fallback = defaultVersion(catalog.data, next, nextFilter);
      if (fallback) {
        setMc(fallback);
        setLoaderVersion(fallback === profile.minecraft ? profile.loaderVersion : null);
      }
    }
  };

  const saveVersion = () => {
    if (!canSave || !selectedMc) return;
    const seq = ++saveSeqRef.current;
    setSaving(true);
    setSubmitError(null);
    const choice =
      moddedLoader && chosenLoaderVersion
        ? ({ mode: "pinned", version: chosenLoaderVersion } as const)
        : null;
    api.setVersion(profile.id, selectedMc, choice).then(
      (snapshot) => {
        if (!mountedRef.current || !activeRef.current || saveSeqRef.current !== seq) return;
        playSound("primary");
        setSaving(false);
        onSaved(snapshot);
      },
      (err: unknown) => {
        if (!mountedRef.current || !activeRef.current || saveSeqRef.current !== seq) return;
        setSaving(false);
        setSubmitError(describeError("profile", err));
      },
    );
  };

  const loaderIssue = loaderFailed
    ? t("create.loaderCheckFailed")
    : currentLoader && currentLoader.status !== "ok"
      ? currentLoader.status === "none"
        ? selectedMc
          ? t("create.loaderNone", {
              loader: LOADER_NAMES[moddedLoader!],
              minecraft: selectedMc,
            })
          : t("create.loaderNoneAny", { loader: LOADER_NAMES[moddedLoader!] })
        : t("create.loaderUnchecked", { loader: LOADER_NAMES[moddedLoader!] })
      : null;

  return (
    <div
      ref={editorRef}
      tabIndex={-1}
      className="bg-sand/35 ring-graphite/10 mt-0.5 flex flex-col gap-1 rounded-2xl p-2 ring-1 outline-hidden forced-colors:border forced-colors:border-[CanvasText]"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
          <span id="profile-edit-mc-label" className={LABEL}>
            {t("create.mcVersion")}
          </span>
          {isAdvanced && (
            <Segmented
              variant="plain"
              disabled={running || saving}
              label={t("create.filterLabel")}
              value={filter}
              onChange={(f) => {
                playSound("click");
                changeFilter(f);
              }}
              options={(Object.keys(FILTER_KEYS) as VersionFilter[]).map((f) => ({
                value: f,
                label: t(FILTER_KEYS[f]),
              }))}
            />
          )}
        </div>
        {catalog.status === "ready" ? (
          <Select
            value={selectedMc}
            disabled={running || saving || shown.length === 0}
            onValueChange={(v) => {
              if (typeof v === "string") {
                playSound("select");
                setMc(v);
                setLoaderVersion(v === profile.minecraft ? profile.loaderVersion : null);
                setSubmitError(null);
              }
            }}
            items={shown.map((e) => ({ value: e.id, label: e.id }))}
          >
            <SelectTrigger
              ref={mcTriggerRef}
              aria-labelledby="profile-edit-mc-label"
              className={cn(FIELD, "h-9 min-w-0 justify-between py-0 pr-3")}
            >
              <SelectValue placeholder={t("create.choose")} />
            </SelectTrigger>
            <SelectContent
              alignItemWithTrigger={false}
              sideOffset={6}
              className="max-h-[min(15rem,var(--available-height))] rounded-xl p-1"
            >
              {shown.map((e) => (
                <SelectItem key={e.id} value={e.id} className="rounded-lg py-1.5">
                  <span className="flex-1">{e.id}</span>
                  <span className="text-cream/75 pr-4 text-[0.75rem] tabular-nums">
                    {releaseDay(e.releaseTime)}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <div className={cn(FIELD, "text-ink-muted flex h-9 items-center")} aria-live="polite">
            {catalog.status === "loading" ? t("create.loading") : t("create.unavailable")}
          </div>
        )}
        {catalog.status === "error" && (
          <p role="alert" className="text-clay-deep text-[0.8125rem]">
            {catalog.message}{" "}
            <button
              type="button"
              className="focus-visible:ring-ring underline underline-offset-2 outline-hidden focus-visible:ring-2"
              onClick={() => {
                playSound("click");
                pendingCatalogFocusRef.current = true;
                setReload((n) => n + 1);
              }}
            >
              {t("common.tryAgain")}
            </button>
          </p>
        )}
        {catalog.status === "ready" && catalog.data.stale && (
          <p className="text-ink-muted text-[0.8125rem]">{staleNote(catalog.data.fetchedAt)}</p>
        )}
        {catalog.status === "ready" && shown.length === 0 && (
          <p className="text-ink-muted text-[0.8125rem]">{t("create.noVersions")}</p>
        )}
      </div>

      {moddedLoader &&
        (isAdvanced ? (
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
              <span id="profile-edit-loader-label" className={LABEL}>
                {t("profileModal.loaderVersionFor", { loader: LOADER_NAMES[moddedLoader] })}
              </span>
              {loaderIssue && (
                <span role="alert" className="text-clay-deep text-[0.75rem]">
                  {loaderIssue}
                </span>
              )}
            </div>
            <Select
              value={chosenLoaderVersion}
              disabled={
                running ||
                saving ||
                catalog.status === "error" ||
                !currentLoader ||
                currentLoader.status !== "ok"
              }
              onValueChange={(v) => {
                if (typeof v === "string") {
                  playSound("select");
                  setLoaderVersion(v);
                  setSubmitError(null);
                }
              }}
              items={(currentLoader?.versions ?? []).map((v) => ({
                value: v.version,
                label:
                  v.version === currentLoader?.recommended
                    ? t("create.versionRecommended", { version: v.version })
                    : v.version,
              }))}
            >
              <SelectTrigger
                aria-labelledby="profile-edit-loader-label"
                className={cn(FIELD, "h-9 min-w-0 justify-between py-0 pr-3 disabled:opacity-50")}
              >
                <SelectValue
                  placeholder={loadingLoaders ? t("create.checking") : t("create.noCompatible")}
                />
              </SelectTrigger>
              <SelectContent
                alignItemWithTrigger={false}
                sideOffset={6}
                className="max-h-[min(14rem,var(--available-height))] rounded-xl p-1"
              >
                {(currentLoader?.versions ?? []).map((v) => (
                  <SelectItem key={v.version} value={v.version} className="rounded-lg py-1.5">
                    <span className="flex-1">{v.version}</span>
                    <span className="text-cream/75 pr-4 text-[0.75rem]">
                      {v.version === currentLoader?.recommended
                        ? t("create.channel.recommended")
                        : t(CHANNEL_NOTE[v.channel])}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        ) : loaderIssue ? (
          <p role="alert" className="text-clay-deep text-[0.75rem]">
            {loaderIssue}
          </p>
        ) : null)}

      <p className="text-ink-muted text-[0.75rem] leading-snug">
        {moddedLoader ? t("profileModal.versionModdedNote") : t("profileModal.versionKeepNote")}
      </p>

      {submitError && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {submitError}
        </p>
      )}

      <div className="flex items-center justify-end gap-2">
        <button
          type="button"
          disabled={running || saving}
          onClick={onCancel}
          className={BUTTON_SMALL_QUIET}
        >
          {t("common.cancel")}
        </button>
        <button
          type="button"
          disabled={!canSave}
          onClick={saveVersion}
          className={BUTTON_SMALL_PRIMARY}
        >
          {saving ? t("profileModal.saving") : t("profileModal.saveVersion")}
        </button>
      </div>
    </div>
  );
}
