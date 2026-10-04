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

/**
 * Setting a profile up: one calm surface (the shared Modal) showing the real stage, real counts
 * and, only when the backend can measure it, a real progress bar. There is no downloads
 * dashboard. When it finishes it says so; it never pretends the game started.
 */
export function PrepareDialog({ profile, verify = false, onClose, onPrepared, api }: Props) {
  // While preparing, closing the modal (Escape) cancels it; files already downloaded are kept.
  const cancelRef = useRef<(() => void) | null>(null);
  const [shown, setShown] = useState(profile);
  if (profile && profile !== shown && profile.id !== shown?.id) setShown(profile);
  const current = profile ?? shown;

  return (
    <Modal
      open={profile !== null}
      onOpenChange={(o) => {
        if (!o) {
          cancelRef.current?.();
          onClose();
        }
      }}
      size="sm"
      closeOnBackdrop={false}
    >
      {current && (
        <Run
          key={current.id}
          profile={current}
          verify={verify}
          api={api}
          cancelRef={cancelRef}
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
  onClose,
  onPrepared,
}: Pick<Props, "api" | "onClose" | "onPrepared"> & {
  verify: boolean;
  profile: ProfileSummary;
  cancelRef: React.RefObject<(() => void) | null>;
}) {
  const t = useT();
  const [phase, setPhase] = useState<Phase>({ kind: "working", progress: null });

  const start = () => {
    let stop: (() => void) | null = null;
    let finished = false;
    void api
      .onPrepareProgress((p) => {
        if (p.instanceId === profile.id)
          setPhase((cur) => (cur.kind === "working" ? { kind: "working", progress: p } : cur));
      })
      .then((u) => {
        if (finished) u();
        else stop = u;
      });
    cancelRef.current = () => void api.cancelPrepare().catch(() => undefined);
    (verify ? api.prepare(profile.id, { verify: true }) : api.prepare(profile.id)).then(
      (snapshot) => {
        finished = true;
        stop?.();
        cancelRef.current = null;
        playSound("primary");
        onPrepared(snapshot);
        setPhase({ kind: "done" });
      },
      (e: unknown) => {
        finished = true;
        stop?.();
        cancelRef.current = null;
        if (e instanceof BackendError && e.code === "cancelled") return onClose();
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
              className="bg-clay h-full rounded-full transition-[width] duration-200 ease-out"
              style={{ width: `${Math.max(2, p.overall * 100)}%` }}
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
