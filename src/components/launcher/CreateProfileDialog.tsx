import { useEffect, useMemo, useRef, useState, type RefObject } from "react";
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
  MAX_NAME_CHARS,
  releaseDay,
  staleNote,
  validateName,
  type VersionFilter,
} from "@/features/launcher/profileUi";
import { useT, type MessageKey } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import {
  LOADER_NAMES,
  LOADER_ORDER,
  profilesApi,
  type LoaderKind,
  type LoaderOption,
  type NewProfileRequest,
  type ProfilesApi,
  type ProfilesSnapshot,
  type VersionCatalog,
} from "@/lib/profiles/api";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_PRIMARY, BUTTON_QUIET, FIELD, LABEL } from "./modalStyles";
import { Segmented } from "./Segmented";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Names of existing profiles, for instant duplicate feedback (the backend checks again). */
  existingNames: readonly string[];
  onCreated: (snapshot: ProfilesSnapshot) => void;
  /** Injectable for tests; the real backend by default. */
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

/** The dialog shell. The form inside is mounted only while open, so every opening starts fresh. */
export function CreateProfileDialog({
  open,
  onOpenChange,
  existingNames,
  onCreated,
  api = profilesApi,
}: Props) {
  const nameRef = useRef<HTMLInputElement>(null);

  return (
    <Modal open={open} onOpenChange={onOpenChange} initialFocus={nameRef} closeOnBackdrop={false}>
      <ProfileForm
        nameRef={nameRef}
        existingNames={existingNames}
        api={api}
        onCancel={() => {
          playSound("click");
          onOpenChange(false);
        }}
        onCreated={(snapshot) => {
          onCreated(snapshot);
          onOpenChange(false);
        }}
      />
    </Modal>
  );
}

interface FormProps {
  nameRef: RefObject<HTMLInputElement | null>;
  existingNames: readonly string[];
  api: ProfilesApi;
  onCancel: () => void;
  onCreated: (snapshot: ProfilesSnapshot) => void;
}

function ProfileForm({ nameRef, existingNames, api, onCancel, onCreated }: FormProps) {
  const t = useT();
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [filter, setFilter] = useState<VersionFilter>("releases");
  const [reload, setReload] = useState(0);
  const [catalogResult, setCatalogResult] = useState<{
    key: number;
    state: Exclude<CatalogState, { status: "loading" }>;
  } | null>(null);
  const [mc, setMc] = useState<string | null>(null);
  const [type, setType] = useState<"vanilla" | "modded">("vanilla");
  const [loader, setLoader] = useState<LoaderKind | null>(null);
  const [loaderVersion, setLoaderVersion] = useState<string | null>(null); // null = the recommended one
  const [loaderResult, setLoaderResult] = useState<{
    mc: string;
    options: LoaderOption[] | null;
  } | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  // "Loading" is derived: a result only counts if it belongs to the current request.
  const catalog = useMemo<CatalogState>(
    () => (catalogResult?.key === reload ? catalogResult.state : { status: "loading" }),
    [catalogResult, reload],
  );

  // The official Minecraft version list (from the backend: live, or the saved copy when offline).
  useEffect(() => {
    let cancelled = false;
    api.versionCatalog().then(
      (data) => {
        if (cancelled) return;
        setCatalogResult({ key: reload, state: { status: "ready", data } });
        setMc(defaultVersion(data, filterVersions(data.entries, "releases"), "releases"));
      },
      (e: unknown) => {
        if (!cancelled)
          setCatalogResult({
            key: reload,
            state: {
              status: "error",
              message: describeError("catalog", e),
            },
          });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [reload, api]);

  // Compatible loaders for the chosen Minecraft version, only while the profile is Modded.
  useEffect(() => {
    if (type !== "modded" || !mc) return;
    let cancelled = false;
    api.loaderOptions(mc).then(
      (options) => {
        if (cancelled) return;
        setLoaderResult({ mc, options });
        setLoader((prev) => {
          const ok = (k: LoaderKind | null) =>
            k !== null && options.some((o) => o.loader === k && o.status === "ok");
          return ok(prev) ? prev : (LOADER_ORDER.find((k) => ok(k)) ?? null);
        });
        setLoaderVersion(null);
      },
      () => {
        if (!cancelled) setLoaderResult({ mc, options: null });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [type, mc, api]);

  const entries = useMemo(
    () => (catalog.status === "ready" ? catalog.data.entries : []),
    [catalog],
  );
  const shown = useMemo(() => filterVersions(entries, filter), [entries, filter]);
  const loaderForMc = loaderResult?.mc === mc ? loaderResult : null; // a result for another version does not count
  const options = loaderForMc?.options ?? [];
  const loaderFailed = loaderForMc !== null && loaderForMc.options === null;
  const loadingLoaders = type === "modded" && !!mc && loaderForMc === null;
  const current = loader ? options.find((o) => o.loader === loader) : undefined;
  const chosenLoaderVersion = current
    ? loaderVersion && current.versions.some((v) => v.version === loaderVersion)
      ? loaderVersion
      : current.recommended
    : null;

  const nameError = validateName(name, existingNames);
  const showNameError = nameTouched && nameError;
  const canSubmit =
    !submitting &&
    !nameError &&
    catalog.status === "ready" &&
    !!mc &&
    (type === "vanilla" ||
      (!loadingLoaders && !!loader && !!chosenLoaderVersion && current?.status === "ok"));

  const changeFilter = (f: VersionFilter) => {
    setFilter(f);
    if (catalog.status !== "ready") return;
    const next = filterVersions(catalog.data.entries, f);
    if (!mc || !next.some((e) => e.id === mc)) setMc(defaultVersion(catalog.data, next, f));
  };

  const submit = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    setNameTouched(true);
    if (!canSubmit || !mc) return;
    const request: NewProfileRequest = {
      name: name.trim(),
      minecraft: mc,
      profile:
        type === "vanilla"
          ? { type: "vanilla" }
          : {
              type: "modded",
              loader: loader as LoaderKind,
              loader_version: { mode: "pinned", version: chosenLoaderVersion as string },
            },
    };
    setSubmitting(true);
    setSubmitError(null);
    api.create(request).then(
      (snapshot) => {
        playSound("primary");
        onCreated(snapshot);
      },
      (err: unknown) => {
        setSubmitting(false);
        setSubmitError(describeError("profile", err));
      },
    );
  };

  const loaderHints = options
    .filter((o) => o.status !== "ok")
    .map((o) =>
      o.status === "none"
        ? mc
          ? t("create.loaderNone", { loader: LOADER_NAMES[o.loader], minecraft: mc })
          : t("create.loaderNoneAny", { loader: LOADER_NAMES[o.loader] })
        : t("create.loaderUnchecked", { loader: LOADER_NAMES[o.loader] }),
    );

  return (
    <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
      <ModalHeader title={t("create.title")} description={t("create.description")} />

      {/* Name */}
      <div className="flex flex-col gap-1.5">
        <label htmlFor="profile-name" className={LABEL}>
          {t("create.name")}
        </label>
        <input
          ref={nameRef}
          id="profile-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => setNameTouched(true)}
          autoComplete="off"
          spellCheck={false}
          maxLength={80}
          placeholder={t("create.namePlaceholder")}
          aria-invalid={showNameError ? true : undefined}
          aria-describedby={showNameError ? "profile-name-error" : undefined}
          className={FIELD}
        />
        <p
          id="profile-name-error"
          role={showNameError ? "alert" : undefined}
          className={`text-clay-deep min-h-4 text-[0.8125rem] leading-4 transition-opacity duration-200 ${showNameError ? "opacity-100" : "opacity-0"}`}
        >
          {showNameError ? t(nameError, { max: MAX_NAME_CHARS }) : ""}
        </p>
      </div>

      {/* Minecraft version */}
      <div className="flex min-w-0 flex-col gap-1.5">
        <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
          <span id="mc-version-label" className={LABEL}>
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
              }
            }}
            items={shown.map((e) => ({ value: e.id, label: e.id }))}
          >
            <SelectTrigger
              aria-labelledby="mc-version-label"
              className={`${FIELD} min-w-0 justify-between border-0 py-0 pr-3`}
            >
              <SelectValue placeholder={t("create.choose")} />
            </SelectTrigger>
            <SelectContent
              alignItemWithTrigger={false}
              sideOffset={6}
              className="max-h-72 rounded-xl p-1"
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
          <div className={`${FIELD} text-ink-muted flex items-center`} aria-live="polite">
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

      {/* Profile type */}
      <div className="flex flex-col gap-1.5">
        <span id="profile-type-label" className={LABEL}>
          {t("create.type")}
        </span>
        <Segmented
          className="self-start"
          label={t("create.typeLabel")}
          value={type}
          onChange={(t) => {
            playSound("click");
            setType(t);
          }}
          options={[
            { value: "vanilla", label: t("profile.vanilla") },
            { value: "modded", label: t("profile.modded") },
          ]}
        />
        <p className="text-ink-muted text-[0.8125rem]">
          {type === "vanilla" ? t("create.vanillaNote") : t("create.moddedNote")}
        </p>
      </div>

      {/* Modded only: revealed smoothly, and unreachable while hidden */}
      <div className="reveal" data-open={type === "modded"} inert={type !== "modded"}>
        <div className="reveal-inner">
          <div className="flex flex-col gap-4 pt-0.5 pb-1">
            <div className="flex min-w-0 flex-col gap-1.5">
              <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <span className={LABEL}>{t("create.loader")}</span>
                <span
                  className="text-ink-muted text-right text-[0.75rem] leading-4"
                  aria-live="polite"
                >
                  {loaderFailed
                    ? t("create.loaderCheckFailed")
                    : loadingLoaders
                      ? t("create.loaderChecking")
                      : loaderHints.length > 0
                        ? `${loaderHints.join(". ")}.`
                        : ""}
                </span>
              </div>
              <Segmented
                className="self-start"
                label={t("create.loaderLabel")}
                value={loader}
                onChange={(k) => {
                  playSound("click");
                  setLoader(k);
                  setLoaderVersion(null);
                }}
                options={LOADER_ORDER.map((k) => ({
                  value: k,
                  label: LOADER_NAMES[k],
                  disabled: loadingLoaders || options.find((o) => o.loader === k)?.status !== "ok",
                }))}
              />
            </div>

            <div className="flex min-w-0 flex-col gap-1.5">
              <span id="loader-version-label" className={LABEL}>
                {t("create.loaderVersion")}
              </span>
              <Select
                value={chosenLoaderVersion}
                disabled={!current || current.status !== "ok"}
                onValueChange={(v) => {
                  if (typeof v === "string") {
                    playSound("select");
                    setLoaderVersion(v);
                  }
                }}
                items={(current?.versions ?? []).map((v) => ({
                  value: v.version,
                  label:
                    v.version === current?.recommended
                      ? t("create.versionRecommended", { version: v.version })
                      : v.version,
                }))}
              >
                <SelectTrigger
                  aria-labelledby="loader-version-label"
                  className={`${FIELD} min-w-0 justify-between border-0 py-0 pr-3 disabled:opacity-50`}
                >
                  <SelectValue
                    placeholder={loadingLoaders ? t("create.checking") : t("create.noCompatible")}
                  />
                </SelectTrigger>
                <SelectContent
                  alignItemWithTrigger={false}
                  sideOffset={6}
                  className="max-h-60 rounded-xl p-1"
                >
                  {(current?.versions ?? []).map((v) => (
                    <SelectItem key={v.version} value={v.version} className="rounded-lg py-1.5">
                      <span className="flex-1">{v.version}</span>
                      <span className="text-cream/45 pr-4 text-[0.75rem]">
                        {v.version === current?.recommended
                          ? t("create.channel.recommended")
                          : t(CHANNEL_NOTE[v.channel])}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {current?.stale && (
                <p className="text-ink-muted text-[0.8125rem]">{t("create.staleLoaders")}</p>
              )}
            </div>
          </div>
        </div>
      </div>

      {submitError && (
        <p role="alert" className="text-clay-deep text-[0.875rem]">
          {submitError}
        </p>
      )}

      <ModalFooter>
        <button type="button" onClick={onCancel} className={BUTTON_QUIET}>
          {t("common.cancel")}
        </button>
        <button type="submit" disabled={!canSubmit} className={BUTTON_PRIMARY}>
          {submitting ? t("create.creating") : t("create.submit")}
        </button>
      </ModalFooter>
    </form>
  );
}
