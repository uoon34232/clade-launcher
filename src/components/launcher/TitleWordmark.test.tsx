// The title wordmark and the sound of the Settings "Done" button.
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { setMotionMode } from "@/lib/motion";

const playSound = vi.fn<(kind: string) => boolean>(() => true);
vi.mock("@/lib/sound/soundService", () => ({
  playSound: (kind: string) => playSound(kind),
  sound: {
    getSettings: () => ({ enabled: true, volume: 0.5 }),
    subscribe: () => () => undefined,
    setSettings: () => undefined,
  },
}));

import { SettingsDialog } from "./SettingsDialog";
import { TitleWordmark } from "./TitleWordmark";

beforeEach(() => {
  vi.useFakeTimers();
  setLocale("en");
  setMotionMode("off");
  playSound.mockClear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  setMotionMode("system");
});

const wordmark = () => screen.getByRole("button", { name: t("window.wordmark") });

describe("the title wordmark", () => {
  it("is a quiet, focusable button that starts at rest", () => {
    render(<TitleWordmark />);
    expect(wordmark().getAttribute("data-phase")).toBe("off");
    expect(wordmark().tabIndex).toBe(0);
    expect(wordmark().textContent).toContain("Clade");
  });

  it("shows the brand phrase for about a second, then settles back, and can repeat", () => {
    render(<TitleWordmark />);
    fireEvent.click(wordmark());
    expect(wordmark().getAttribute("data-phase")).toBe("on");
    expect(wordmark().getAttribute("data-mode")).toBe("full");
    act(() => void vi.advanceTimersByTime(900));
    expect(wordmark().getAttribute("data-phase")).toBe("on");
    act(() => void vi.advanceTimersByTime(400));
    expect(wordmark().getAttribute("data-phase")).toBe("off");
    fireEvent.click(wordmark());
    expect(wordmark().getAttribute("data-phase")).toBe("on");
  });

  it("uses the plain crossfade with reduced motion", () => {
    setMotionMode("on");
    render(<TitleWordmark />);
    fireEvent.click(wordmark());
    expect(wordmark().getAttribute("data-mode")).toBe("reduced");
  });

  it("plays no sound and the brand phrase comes from the catalog in both languages", () => {
    render(<TitleWordmark />);
    fireEvent.click(wordmark());
    expect(playSound).not.toHaveBeenCalled();
    expect(wordmark().textContent).toContain(t("onboarding.tagline"));
    cleanup();
    setLocale("ru");
    render(<TitleWordmark />);
    expect(wordmark().textContent).toContain("Просто. Портативно. Твоё.");
  });

  it("sizes the clay underline to 100% of the tagline wrapper", () => {
    render(<TitleWordmark />);
    const line = wordmark().querySelector(".wordmark-line");
    const tagline = wordmark().querySelector(".wordmark-tagline");
    expect(line).toBeTruthy();
    expect(tagline).toBeTruthy();
    expect(line?.className).toContain("w-full");
    expect(line?.parentElement).toBe(tagline?.parentElement);
  });
});

describe("Settings Done", () => {
  it("plays the normal click once and closes", () => {
    const onOpenChange = vi.fn();
    render(<SettingsDialog open onOpenChange={onOpenChange} />);
    fireEvent.click(screen.getByRole("button", { name: t("common.done") }));
    expect(playSound.mock.calls.filter(([k]) => k === "click")).toHaveLength(1);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
