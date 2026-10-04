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

import { OnboardingDialog } from "./OnboardingDialog";

beforeEach(() => setLocale("en"));
afterEach(cleanup);

const button = (
  key: "onboarding.skip" | "onboarding.continue" | "onboarding.finish" | "onboarding.back",
) => screen.getByRole("button", { name: t(key) });
const maybe = (key: "onboarding.skip" | "onboarding.finish") =>
  screen.queryByRole("button", { name: t(key) });

describe("onboarding buttons", () => {
  it("never reuse one action's DOM node for another across the pages", () => {
    render(<OnboardingDialog open onExit={() => undefined} />);
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
    expect(skipAgain.className).not.toContain("bg-clay");
    expect(maybe("onboarding.finish")).toBeNull();
  });
});
