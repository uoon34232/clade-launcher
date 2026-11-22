// Skip, Continue and Finish are different actions: React must never hand one button's DOM node to
// another, or the old colours animate into the new ones (a clay flash on Skip after Back).
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";

vi.mock("@/lib/sound/soundService", () => ({
  playSound: vi.fn(() => true),
  sound: {
    getSettings: () => ({ enabled: true, volume: 0.5 }),
    subscribe: () => () => undefined,
    setSettings: () => undefined,
  },
}));

import { playSound } from "@/lib/sound/soundService";
import { OnboardingDialog, type OnboardingExit } from "./OnboardingDialog";

const playSoundMock = vi.mocked(playSound);

beforeEach(() => {
  setLocale("en");
  vi.clearAllMocks();
});
afterEach(cleanup);

type OnboardingButtonKey =
  | "onboarding.skip"
  | "onboarding.continue"
  | "onboarding.finish"
  | "onboarding.back"
  | "onboarding.createProfile"
  | "onboarding.addAccount";

const button = (key: OnboardingButtonKey) => screen.getByRole("button", { name: t(key) });
const maybe = (key: OnboardingButtonKey) => screen.queryByRole("button", { name: t(key) });

function renderDialog(open = true) {
  const onExit = vi.fn<(how: OnboardingExit) => void>();
  const view = render(<OnboardingDialog open={open} onExit={onExit} />);
  return { onExit, ...view };
}

describe("OnboardingDialog", () => {
  it("renders nothing when open is false", () => {
    const { onExit } = renderDialog(false);

    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(onExit).not.toHaveBeenCalled();
    expect(playSoundMock).not.toHaveBeenCalled();
  });

  it("never reuses one action's DOM node for another across the pages", () => {
    renderDialog();
    const skip1 = button("onboarding.skip");
    const continue1 = button("onboarding.continue");

    fireEvent.click(continue1);
    expect(button("onboarding.skip")).toBe(skip1); // same action, same node
    fireEvent.click(button("onboarding.continue"));
    const finish = button("onboarding.finish");
    expect(finish).not.toBe(skip1);
    expect(finish).not.toBe(continue1);
    expect(maybe("onboarding.skip")).toBeNull();

    fireEvent.click(button("onboarding.back"));
    const skipAgain = button("onboarding.skip");
    expect(skipAgain).not.toBe(finish);
    expect(skipAgain).not.toBe(continue1);
    expect(maybe("onboarding.finish")).toBeNull();
  });

  it("navigates forward and back across steps with click sound cues", () => {
    const { onExit } = renderDialog();

    expect(screen.getByText(t("onboarding.step", { current: 1, total: 3 }))).toBeTruthy();
    expect(screen.getByRole("heading", { name: t("onboarding.welcomeTitle") })).toBeTruthy();
    expect(maybe("onboarding.back")).toBeNull();
    expect(maybe("onboarding.skip")).not.toBeNull();
    expect(maybe("onboarding.continue")).not.toBeNull();
    expect(maybe("onboarding.finish")).toBeNull();

    fireEvent.click(button("onboarding.continue"));
    expect(screen.getByText(t("onboarding.step", { current: 2, total: 3 }))).toBeTruthy();
    expect(screen.getByRole("heading", { name: t("onboarding.portableTitle") })).toBeTruthy();
    expect(maybe("onboarding.back")).not.toBeNull();

    fireEvent.click(button("onboarding.back"));
    expect(screen.getByText(t("onboarding.step", { current: 1, total: 3 }))).toBeTruthy();
    expect(maybe("onboarding.back")).toBeNull();
    expect(document.activeElement).toBe(button("onboarding.continue"));

    fireEvent.click(button("onboarding.continue"));
    fireEvent.click(button("onboarding.continue"));
    expect(screen.getByText(t("onboarding.step", { current: 3, total: 3 }))).toBeTruthy();
    expect(screen.getByRole("heading", { name: t("onboarding.readyTitle") })).toBeTruthy();
    expect(maybe("onboarding.back")).not.toBeNull();
    expect(maybe("onboarding.skip")).toBeNull();
    expect(maybe("onboarding.continue")).toBeNull();
    expect(maybe("onboarding.createProfile")).not.toBeNull();
    expect(maybe("onboarding.addAccount")).not.toBeNull();
    expect(maybe("onboarding.finish")).not.toBeNull();

    fireEvent.click(button("onboarding.back"));
    expect(screen.getByText(t("onboarding.step", { current: 2, total: 3 }))).toBeTruthy();
    expect(maybe("onboarding.finish")).toBeNull();
    expect(maybe("onboarding.createProfile")).toBeNull();
    expect(maybe("onboarding.addAccount")).toBeNull();
    expect(maybe("onboarding.skip")).not.toBeNull();
    expect(maybe("onboarding.continue")).not.toBeNull();

    expect(playSoundMock).toHaveBeenCalledTimes(5);
    expect(playSoundMock.mock.calls).toEqual([
      ["click"],
      ["click"],
      ["click"],
      ["click"],
      ["click"],
    ]);
    expect(onExit).not.toHaveBeenCalled();
  });

  it.each([0, 1])('exits with "skip" and plays the click sound on step %i', (step) => {
    const { onExit } = renderDialog();
    for (let i = 0; i < step; i++) {
      fireEvent.click(button("onboarding.continue"));
    }
    playSoundMock.mockClear();

    fireEvent.click(button("onboarding.skip"));

    expect(onExit).toHaveBeenCalledTimes(1);
    expect(onExit).toHaveBeenCalledWith("skip");
    expect(playSoundMock).toHaveBeenCalledTimes(1);
    expect(playSoundMock).toHaveBeenCalledWith("click");
  });

  it.each([
    ["onboarding.finish", "finish"],
    ["onboarding.createProfile", "create-profile"],
    ["onboarding.addAccount", "add-account"],
  ] as const)(
    "exits with %s -> %s and plays the primary sound on the last step",
    (buttonKey, expectedExit) => {
      const { onExit } = renderDialog();
      fireEvent.click(button("onboarding.continue"));
      fireEvent.click(button("onboarding.continue"));
      playSoundMock.mockClear();

      fireEvent.click(button(buttonKey));

      expect(onExit).toHaveBeenCalledTimes(1);
      expect(onExit).toHaveBeenCalledWith(expectedExit);
      expect(playSoundMock).toHaveBeenCalledTimes(1);
      expect(playSoundMock).toHaveBeenCalledWith("primary");
    },
  );

  it('exits with "skip" and plays the click sound when Escape is pressed', () => {
    const { onExit } = renderDialog();

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });

    expect(onExit).toHaveBeenCalledTimes(1);
    expect(onExit).toHaveBeenCalledWith("skip");
    expect(playSoundMock).toHaveBeenCalledTimes(1);
    expect(playSoundMock).toHaveBeenCalledWith("click");
  });

  it('exits with "skip" and plays the click sound when Escape is pressed on the last step', () => {
    const { onExit } = renderDialog();
    fireEvent.click(button("onboarding.continue"));
    fireEvent.click(button("onboarding.continue"));
    playSoundMock.mockClear();

    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });

    expect(onExit).toHaveBeenCalledTimes(1);
    expect(onExit).toHaveBeenCalledWith("skip");
    expect(playSoundMock).toHaveBeenCalledTimes(1);
    expect(playSoundMock).toHaveBeenCalledWith("click");
  });

  it("does not exit or play a sound when the backdrop is clicked", () => {
    const { onExit } = renderDialog();
    const overlay = document.querySelector('[data-slot="dialog-overlay"]');
    if (!overlay) throw new Error("Expected dialog overlay to be rendered");

    fireEvent.pointerDown(overlay, { button: 0 });
    fireEvent.mouseDown(overlay, { button: 0 });
    fireEvent.click(overlay);

    expect(onExit).not.toHaveBeenCalled();
    expect(playSoundMock).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeTruthy();
  });
});
