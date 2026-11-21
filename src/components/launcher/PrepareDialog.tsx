import { useEffect, useRef, useState } from "react";
import { formatBytes, itemsText, profileDetail } from "@/features/launcher/profileUi";
import { stageText } from "@/features/launcher/prepareUi";
import { useT } from "@/lib/i18n";
import { describeError } from "@/lib/i18n/errors";
import { BackendError } from "@/lib/ipc";
import type {
  PrepareProgress,
  ProfileSummary,
  ProfilesApi,
  ProfilesSnapshot,
} from "@/lib/profiles/api";
import { playSound } from "@/lib/sound/soundService";
import { Modal, ModalFooter, ModalHeader } from "./Modal";
import { BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT } from "./modalStyles";

interface Props {
  /** The profile being set up; `null` keeps the dialog closed. */
  profile: ProfileSummary | null;
  /** Re-check every file instead of trusting what is there (also repairs). */
  verify?: boolean;
  onClose: () => void;
  onPrepared: (snapshot: ProfilesSnapshot) => void;
  api: ProfilesApi;
}

type Phase =
  | { kind: "working"; progress: PrepareProgress | null }
  | { kind: "done" }
  | { kind: "error"; message: string };

function sameProgress(a: PrepareProgress | null, b: PrepareProgress): boolean {
  if (!a) return false;
  const aLabel = "label" in a ? (a as { label?: unknown }).label : undefined;
  const bLabel = "label" in b ? (b as { label?: unknown }).label : undefined;
  const sameOverall =
    a.overall === b.overall ||
    (a.overall !== null && b.overall !== null && Math.abs(a.overall - b.overall) < 1e-4);
  return (
    a.stage === b.stage &&
    aLabel === bLabel &&
    a.itemsDone === b.itemsDone &&
    a.itemsTotal === b.itemsTotal &&
    a.bytesDone === b.bytesDone &&
    a.bytesTotal === b.bytesTotal &&
    sameOverall
  );
}

/**
 * Setting a profile up: one calm surface (the shared Modal) showing the real stage, real counts
 * and, only when the backend can measure it, a real progress bar. There is no downloads
 * dashboard. When it finishes it says so; it never pretends the game started.
 */
export function PrepareDialog({ profile, verify = false, onClose, onPrepared, api }: Props) {
  // While preparing, closing the modal (Escape) cancels it; files already downloaded are kept.
  const cancelRef = useRef<(() => void) | null>(null);
  const dismissedRef = useRef(false);
  const [shown, setShown] = useState(profile);
  const [wasOpen, setWasOpen] = useState(profile !== null);
  const [session, setSession] = useState(0);
  if (profile && profile !== shown) setShown(profile);
  const isOpen = profile !== null;
  if (isOpen !== wasOpen) {
    setWasOpen(isOpen);
    if (isOpen) setSession((n) => n + 1);
  }
  const current = profile ?? shown;

  useEffect(() => {
    if (isOpen) dismissedRef.current = false;
  }, [isOpen]);

  return (
    <Modal
      open={isOpen}
      onOpenChange={(o) => {
        if (!o) {
          dismissedRef.current = true;
          cancelRef.current?.();
          onClose();
        }
      }}
      size="sm"
      closeOnBackdrop={false}
    >
      {current && (
        <Run
          key={`${current.id}:${verify ? 1 : 0}:${session}`}
          profile={current}
          verify={verify}
          api={api}
          cancelRef={cancelRef}
          dismissedRef={dismissedRef}
          onClose={onClose}
          onPrepared={onPrepared}
        />
      )}
    </Modal>
  );
}

function Run({
  profile,
  verify,
  api,
  cancelRef,
  dismissedRef,
  onClose,
  onPrepared,
}: Pick<Props, "api" | "onClose" | "onPrepared"> & {
  verify: boolean;
  profile: ProfileSummary;
  cancelRef: React.RefObject<(() => void) | null>;
  dismissedRef: React.RefObject<boolean>;
}) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: "working", progress: null });
  const mountedRef = useRef(true);
  const rafRef = useRef<number | null>(null);
  const scheduledRef = useRef(false);
  const pendingRef = useRef<PrepareProgress | null>(null);
  const lastRenderedRef = useRef<PrepareProgress | null>(null);

  const clearScheduledFrame = () => {
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    scheduledRef.current = false;
    pendingRef.current = null;
  };

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      clearScheduledFrame();
    };
  }, []);

  const working = phase.kind === "working";
  useEffect(() => {
    if (!working) return;
    let active = true;
    let stop: (() => void) | null = null;
    const commitProgress = (next: PrepareProgress) => {
      if (sameProgress(lastRenderedRef.current, next)) return;
      lastRenderedRef.current = next;
      setPhase((cur) => {
        if (cur.kind !== "working" || sameProgress(cur.progress, next)) return cur;
        return { kind: "working", progress: next };
      });
    };
    const scheduleCoalesceWindow = () => {
      if (scheduledRef.current) return;
      scheduledRef.current = true;
      const id = window.requestAnimationFrame(() => {
        scheduledRef.current = false;
        rafRef.current = null;
        const next = pendingRef.current;
        pendingRef.current = null;
        if (!active || !mountedRef.current || !next) return;
        commitProgress(next);
      });
      if (scheduledRef.current) rafRef.current = id;
    };
    void api
      .onPrepareProgress((p) => {
        if (!active || !mountedRef.current || p.instanceId !== profile.id) return;
        const stageChanged = lastRenderedRef.current?.stage !== p.stage;
        if (!scheduledRef.current || stageChanged) {
          pendingRef.current = null;
          commitProgress(p);
          scheduleCoalesceWindow();
          return;
        }
        pendingRef.current = p;
      })
      .then((u) => {
        if (!active || !mountedRef.current) u();
        else stop = u;
      });
    return () => {
      active = false;
      clearScheduledFrame();
      stop?.();
      stop = null;
    };
  }, [working, api, profile.id]);

  const start = () => {
    clearScheduledFrame();
    lastRenderedRef.current = null;
    const myCancel = () => void api.cancelPrepare().catch(() => undefined);
    cancelRef.current = myCancel;
    (verify ? api.prepare(profile.id, { verify: true }) : api.prepare(profile.id)).then(
      (snapshot) => {
        clearScheduledFrame();
        if (cancelRef.current === myCancel) cancelRef.current = null;
        if (!mountedRef.current || dismissedRef.current) return;
        playSound("primary");
        onPrepared(snapshot);
        setPhase({ kind: "done" });
      },
      (e: unknown) => {
        clearScheduledFrame();
        if (cancelRef.current === myCancel) cancelRef.current = null;
        if (!mountedRef.current) return;
        if (e instanceof BackendError && e.code === "cancelled") {
          if (!dismissedRef.current) onClose();
          return;
        }
        setPhase({ kind: "error", message: describeError("prepare", e) });
      },
    );
  };

  // Begin exactly once per opening. (React re-runs effects in development; a second start would be
  // refused as "another profile is being set up".)
  const started = useRef(false);
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    start();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once per opening
  }, []);

  if (phase.kind === "done") {
    return (
      <>
        <ModalHeader
          title={t("prepare.ready")}
          description={t("prepare.readyDescription", { name: profile.name })}
        />
        <ModalFooter>
          <button
            type="button"
            autoFocus
            className={BUTTON_PRIMARY}
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

  if (phase.kind === "error") {
    return (
      <>
        <ModalHeader title={profile.name} description={profileDetail(profile)} />
        <p role="alert" className="text-graphite mb-5 text-[0.9375rem]">
          {phase.message}
        </p>
        <ModalFooter>
          <button
            type="button"
            className={BUTTON_QUIET}
            onClick={() => {
              playSound("click");
              onClose();
            }}
          >
            {t("common.close")}
          </button>
          <button
            type="button"
            autoFocus
            className={BUTTON_SOFT}
            onClick={() => {
              playSound("click");
              setPhase({ kind: "working", progress: null });
              start();
            }}
          >
            {t("common.tryAgain")}
          </button>
        </ModalFooter>
      </>
    );
  }

  const p = phase.progress;
  const counts = [
    p ? itemsText(p.itemsDone, p.itemsTotal) : null,
    p?.bytesDone != null && p.bytesTotal != null
      ? t("prepare.bytes", { done: formatBytes(p.bytesDone), total: formatBytes(p.bytesTotal) })
      : null,
  ].filter(Boolean);
  return (
    <>
      <ModalHeader title={profile.name} description={profileDetail(profile)} />
      <div className="mb-6 flex flex-col gap-3">
        <p aria-live="polite" className="text-graphite min-h-6 text-[0.9375rem]">
          {stageText(p?.stage ?? null, { verify, loader: profile.loader })}…
        </p>
        <div
          role="progressbar"
          aria-label={t("prepare.progress")}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={p?.overall != null ? Math.round(p.overall * 100) : undefined}
          className="bg-sand/70 h-1.5 w-full overflow-hidden rounded-full"
        >
          {p?.overall != null ? (
            <div
              className="bg-clay h-full w-full origin-left rounded-full transition-transform duration-150 ease-out"
              style={{ transform: `scaleX(${Math.max(0, Math.min(1, p.overall))})` }}
            />
          ) : (
            <div className="indeterminate-bar bg-clay/70 h-full w-1/3 rounded-full" />
          )}
        </div>
        <p className="text-ink-muted min-h-5 text-[0.8125rem]">{counts.join(" · ")}</p>
      </div>
      <ModalFooter>
        <button
          type="button"
          className={BUTTON_QUIET}
          onClick={() => {
            playSound("click");
            cancelRef.current?.();
          }}
        >
          {t("common.cancel")}
        </button>
      </ModalFooter>
    </>
  );
}
