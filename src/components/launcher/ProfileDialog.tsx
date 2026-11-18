import { useEffect, useMemo, useRef, useState } from "react";
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
import { describeError } from "@/lib/i18n/errors";
import {
  LOADER_NAMES,
  profilesApi,
  type LoaderOption,
  type ProfileSummary,
  type ProfilesApi,
  type ProfilesSnapshot,
  type VersionCatalog,
} from "@/lib/profiles/api";
import { useUiSettings } from "@/lib/settings/useUiSettings";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader, ModalSection } from "./Modal";
import {
  BUTTON_DANGER,
  BUTTON_PRIMARY,
  BUTTON_QUIET,
  BUTTON_SMALL_SOFT,
  BUTTON_SOFT,
  FIELD,
  LABEL,
} from "./modalStyles";
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
  onDuplicated: (snapshot: ProfilesSnapshot) => void;
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
  onDuplicated,
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

  const [changingVersion, setChangingVersion] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [filesError, setFilesError] = useState<string | null>(null);

  const duplicateProfile = () => {
    if (running || saving) return;
    playSound("click");
    setSaving(true);
    setDuplicating(true);
    setFilesError(null);
    api.duplicate(profile.id).then(
      (snapshot) => {
        playSound("primary");
        setSaving(false);
        setDuplicating(false);
        onDuplicated(snapshot);
      },
      (err: unknown) => {
        setSaving(false);
        setDuplicating(false);
        setFilesError(describeError("profile", err));
      },
    );
  };

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
                setDirty(next !== profile.name || changingVersion);
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
              type="button"
              disabled={running || saving}
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
            profile={profile}
            running={running}
            saving={saving}
            setSaving={setSaving}
            api={api}
            onCancel={() => {
              playSound("click");
              setChangingVersion(false);
              setDirty(name !== profile.name);
            }}
            onSaved={(snapshot) => {
              setChangingVersion(false);
              setDirty(name !== profile.name);
              onUpdated(snapshot);
            }}
          />
        )}
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
          disabled={running || saving}
          className={BUTTON_SOFT}
          onClick={duplicateProfile}
        >
          {duplicating ? t("profileModal.duplicating") : t("profileModal.duplicate")}
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

interface VersionEditorProps {
  profile: ProfileSummary;
  running: boolean;
  saving: boolean;
  setSaving: (saving: boolean) => void;
  api: ProfilesApi;
  onCancel: () => void;
  onSaved: (snapshot: ProfilesSnapshot) => void;
}

function VersionEditor({
  profile,
  running,
  saving,
  setSaving,
  api,
  onCancel,
  onSaved,
}: VersionEditorProps) {
  const t = useT();
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
    let cancelled = false;
    api.versionCatalog().then(
      (data) => {
        if (cancelled) return;
        const match = data.entries.find((e) => e.id === profile.minecraft);
        const initialFilter = filterForKind(match?.kind);
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
  }, [reload, api, profile.minecraft]);

  const moddedLoader = profile.kind === "modded" ? profile.loader : null;

  useEffect(() => {
    if (!moddedLoader || !mc) return;
    let cancelled = false;
    api.loaderOptions(mc).then(
      (options) => {
        if (!cancelled) setLoaderResult({ mc, options });
      },
      () => {
        if (!cancelled) setLoaderResult({ mc, options: null });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [moddedLoader, mc, api]);

  const entries = useMemo(
    () => (catalog.status === "ready" ? catalog.data.entries : []),
    [catalog],
  );
  const shown = useMemo(() => filterVersions(entries, filter), [entries, filter]);
  const loaderForMc = loaderResult?.mc === mc ? loaderResult : null;
  const loaderFailed = loaderForMc !== null && loaderForMc.options === null;
  const loadingLoaders = moddedLoader !== null && !!mc && loaderForMc === null;
  const currentLoader = moddedLoader
    ? loaderForMc?.options?.find((o) => o.loader === moddedLoader)
    : undefined;
  const chosenLoaderVersion = currentLoader
    ? loaderVersion && currentLoader.versions.some((v) => v.version === loaderVersion)
      ? loaderVersion
      : currentLoader.recommended
    : null;

  const isUnchanged =
    mc === profile.minecraft &&
    (moddedLoader === null ||
      (chosenLoaderVersion !== null && chosenLoaderVersion === profile.loaderVersion));

  const canSave =
    !running &&
    !saving &&
    catalog.status === "ready" &&
    !!mc &&
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
    if (!canSave) return;
    setSaving(true);
    setSubmitError(null);
    const choice =
      moddedLoader && chosenLoaderVersion
        ? ({ mode: "pinned", version: chosenLoaderVersion } as const)
        : null;
    api.setVersion(profile.id, mc, choice).then(
      (snapshot) => {
        playSound("primary");
        setSaving(false);
        onSaved(snapshot);
      },
      (err: unknown) => {
        setSaving(false);
        setSubmitError(describeError("profile", err));
      },
    );
  };

  return (
    <div className="bg-sand/35 ring-graphite/10 mt-1 flex flex-col gap-2.5 rounded-2xl p-3 ring-1">
      <div className="flex min-w-0 flex-col gap-1">
        <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
          <span id="profile-edit-mc-label" className={LABEL}>
            {t("create.mcVersion")}
          </span>
          <Segmented
            variant="plain"
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
        </div>
        {catalog.status === "ready" ? (
          <Select
            value={mc}
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
              aria-labelledby="profile-edit-mc-label"
              className={`${FIELD} h-9 min-w-0 justify-between border-0 py-0 pr-3`}
            >
              <SelectValue placeholder={t("create.choose")} />
            </SelectTrigger>
            <SelectContent
              alignItemWithTrigger={false}
              sideOffset={6}
              className="max-h-60 rounded-xl p-1"
            >
              {shown.map((e) => (
                <SelectItem key={e.id} value={e.id} className="rounded-lg py-1.5">
                  <span className="flex-1">{e.id}</span>
                  <span className="text-cream/45 pr-4 text-[0.75rem] tabular-nums">
                    {releaseDay(e.releaseTime)}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <div className={`${FIELD} text-ink-muted flex h-9 items-center`} aria-live="polite">
            {catalog.status === "loading" ? t("create.loading") : t("create.unavailable")}
          </div>
        )}
        {catalog.status === "error" && (
          <p role="alert" className="text-clay-deep text-[0.8125rem]">
            {catalog.message}{" "}
            <button
              type="button"
              className="focus-visible:ring-clay/45 underline underline-offset-2 outline-none focus-visible:ring-2"
              onClick={() => setReload((n) => n + 1)}
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

      {moddedLoader && (
        <div className="flex min-w-0 flex-col gap-1">
          <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
            <span id="profile-edit-loader-label" className={LABEL}>
              {t("profileModal.loaderVersionFor", { loader: LOADER_NAMES[moddedLoader] })}
            </span>
            {loaderFailed ? (
              <span className="text-clay-deep text-[0.75rem]">{t("create.loaderCheckFailed")}</span>
            ) : currentLoader && currentLoader.status !== "ok" ? (
              <span className="text-clay-deep text-[0.75rem]">
                {currentLoader.status === "none"
                  ? t("create.loaderNone", {
                      loader: LOADER_NAMES[moddedLoader],
                      minecraft: mc,
                    })
                  : t("create.loaderUnchecked", { loader: LOADER_NAMES[moddedLoader] })}
              </span>
            ) : null}
          </div>
          <Select
            value={chosenLoaderVersion}
            disabled={!currentLoader || currentLoader.status !== "ok"}
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
              className={`${FIELD} h-9 min-w-0 justify-between border-0 py-0 pr-3 disabled:opacity-50`}
            >
              <SelectValue
                placeholder={loadingLoaders ? t("create.checking") : t("create.noCompatible")}
              />
            </SelectTrigger>
            <SelectContent
              alignItemWithTrigger={false}
              sideOffset={6}
              className="max-h-56 rounded-xl p-1"
            >
              {(currentLoader?.versions ?? []).map((v) => (
                <SelectItem key={v.version} value={v.version} className="rounded-lg py-1.5">
                  <span className="flex-1">{v.version}</span>
                  <span className="text-cream/45 pr-4 text-[0.75rem]">
                    {v.version === currentLoader?.recommended
                      ? t("create.channel.recommended")
                      : t(CHANNEL_NOTE[v.channel])}
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      )}

      <p className="text-ink-muted text-[0.8125rem]">
        {moddedLoader ? t("profileModal.versionModdedNote") : t("profileModal.versionKeepNote")}
      </p>

      {submitError && (
        <p role="alert" className="text-clay-deep text-[0.8125rem]">
          {submitError}
        </p>
      )}

      <div className="flex items-center justify-end gap-2">
        <button type="button" disabled={saving} onClick={onCancel} className={BUTTON_QUIET}>
          {t("common.cancel")}
        </button>
        <button
          type="button"
          disabled={!canSave}
          onClick={saveVersion}
          className={`${BUTTON_PRIMARY} h-9 px-4 text-[0.8125rem]`}
        >
          {saving ? t("profileModal.saving") : t("profileModal.saveVersion")}
        </button>
      </div>
    </div>
  );
}
