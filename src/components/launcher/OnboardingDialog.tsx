import { useState } from "react";
import { FolderIcon } from "lucide-react";
import { useT } from "@/lib/i18n";
import { playSound } from "@/lib/sound/soundService";
import { CladeMark } from "./CladeMark";
import { Modal, ModalHeader } from "./Modal";
import { BUTTON_PRIMARY, BUTTON_QUIET, BUTTON_SOFT } from "./modalStyles";

/** What the person chose on the last page. */
export type OnboardingExit = "finish" | "skip" | "create-profile" | "add-account";

interface Props {
  open: boolean;
  /** Called once, when the introduction ends in any way; the caller records that it was seen. */
  onExit: (how: OnboardingExit) => void;
}

const STEPS = 3;

/**
 * The first-run introduction: three small pages in the shared modal. It never requires an account
 * or a profile and needs no network. Skip and Finish both end it for good; Escape counts as Skip.
 */
export function OnboardingDialog({ open, onExit }: Props) {
  const t = useT();
  const [step, setStep] = useState(0);
  const go = (next: number) => {
    playSound("click");
    setStep(next);
  };
  const exit = (how: OnboardingExit) => {
    playSound(how === "skip" ? "click" : "primary");
    onExit(how);
  };

  return (
    <Modal open={open} onOpenChange={(o) => !o && onExit("skip")} size="md" closeOnBackdrop={false}>
      <div aria-live="polite" className="min-w-0">
        <div key={step} className="animate-in fade-in-0 duration-300">
          <p className="text-ink-muted mb-5 text-[0.8125rem]">
            {t("onboarding.step", { current: step + 1, total: STEPS })}
          </p>
          {step === 0 && <Welcome />}
          {step === 1 && <Portable />}
          {step === 2 && <Ready />}
        </div>
      </div>

      {step === 2 && (
        <div className="mt-6 flex flex-wrap items-center gap-3">
          <button type="button" className={BUTTON_SOFT} onClick={() => exit("create-profile")}>
            {t("onboarding.createProfile")}
          </button>
          <button type="button" className={BUTTON_SOFT} onClick={() => exit("add-account")}>
            {t("onboarding.addAccount")}
          </button>
        </div>
      )}

      <div className={`${step === 2 ? "mt-5" : "mt-7"} flex items-center justify-between gap-3`}>
        <div>
          {step > 0 && (
            <button type="button" className={BUTTON_QUIET} onClick={() => go(step - 1)}>
              {t("onboarding.back")}
            </button>
          )}
        </div>
        <div className="flex flex-wrap items-center justify-end gap-3">
          {/* Distinct keys: Skip, Continue and Finish are different actions, so React must not reuse
              one button's DOM node for another (it would animate the old colours into the new). */}
          {step < 2 && (
            <button key="skip" type="button" className={BUTTON_QUIET} onClick={() => exit("skip")}>
              {t("onboarding.skip")}
            </button>
          )}
          {step < 2 && (
            <button
              key="continue"
              type="button"
              autoFocus
              className={BUTTON_PRIMARY}
              onClick={() => go(step + 1)}
            >
              {t("onboarding.continue")}
            </button>
          )}
          {step === 2 && (
            <button
              key="finish"
              type="button"
              autoFocus
              className={BUTTON_PRIMARY}
              onClick={() => exit("finish")}
            >
              {t("onboarding.finish")}
            </button>
          )}
        </div>
      </div>
    </Modal>
  );
}

function Welcome() {
  const t = useT();
  return (
    <div className="flex flex-col items-start">
      <CladeMark size={64} className="mb-5 drop-shadow-[0_10px_18px_rgb(176_88_66/0.28)]" />
      <ModalHeader title={t("onboarding.welcomeTitle")} description={t("onboarding.tagline")} />
      <p className="text-graphite text-[0.9375rem]">{t("onboarding.welcomeText")}</p>
    </div>
  );
}

function Portable() {
  const t = useT();
  return (
    <div className="flex flex-col items-start">
      <span
        aria-hidden
        className="bg-clay/12 text-clay-deep mb-5 grid size-16 place-items-center rounded-[1.25rem]"
      >
        <FolderIcon strokeWidth={1.5} className="size-8" />
      </span>
      <ModalHeader title={t("onboarding.portableTitle")} />
      <p className="text-graphite text-[0.9375rem]">{t("onboarding.portableText")}</p>
    </div>
  );
}

function Ready() {
  const t = useT();
  return (
    <div className="flex flex-col items-start">
      <ModalHeader title={t("onboarding.readyTitle")} />
      <p className="text-graphite text-[0.9375rem]">{t("onboarding.readyText")}</p>
    </div>
  );
}
