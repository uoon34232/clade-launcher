import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { WindowChrome } from "./WindowChrome";

const invokeSpy = vi.fn<(cmd: string, args?: unknown) => Promise<void>>(() => Promise.resolve());

beforeEach(() => {
  setLocale("en");
  invokeSpy.mockClear();
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    invoke: invokeSpy,
  };
});

afterEach(() => {
  cleanup();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe("WindowChrome", () => {
  it("renders wordmark, minimize, maximize/restore, and close buttons when framed", async () => {
    const onToggleMaximize = vi.fn();
    const { container, rerender } = render(
      <WindowChrome framed={false} maximized={false} onToggleMaximize={onToggleMaximize} />,
    );

    expect(screen.getByRole("button", { name: t("window.wordmark") })).toBeTruthy();
    expect(screen.getByRole("button", { name: t("window.minimize") })).toBeTruthy();
    expect(screen.queryByRole("button", { name: t("window.maximize") })).toBeNull();
    expect(screen.getByRole("button", { name: t("window.close") })).toBeTruthy();

    rerender(
      <WindowChrome
        framed={false}
        launching
        maximized={false}
        onToggleMaximize={onToggleMaximize}
      />,
    );
    const launchingStrip = container.firstElementChild as HTMLElement;
    expect(launchingStrip.className).toContain("border-graphite/10");

    rerender(<WindowChrome framed closing maximized={false} onToggleMaximize={onToggleMaximize} />);
    const strip = container.firstElementChild as HTMLElement;
    expect(strip.className).toContain("h-[44px]");
    expect(strip.getAttribute("data-tauri-drag-region")).toBe("true");
    expect(screen.getByRole("button", { name: t("window.close") }).className).toContain(
      "chrome-closing-pulse",
    );

    const maxBtn = screen.getByRole("button", { name: t("window.maximize") });
    maxBtn.focus();
    expect(document.activeElement).toBe(maxBtn);
    fireEvent.click(maxBtn);
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);
    expect(document.activeElement).not.toBe(maxBtn);

    fireEvent.mouseDown(strip, { button: 0, detail: 2 });
    expect(onToggleMaximize).toHaveBeenCalledTimes(2);

    rerender(<WindowChrome framed maximized onToggleMaximize={onToggleMaximize} />);
    expect(strip.hasAttribute("data-tauri-drag-region")).toBe(false);
    const restoreBtn = screen.getByRole("button", { name: t("window.restore") });
    restoreBtn.focus();
    fireEvent.click(restoreBtn);
    expect(onToggleMaximize).toHaveBeenCalledTimes(3);
    expect(document.activeElement).not.toBe(restoreBtn);

    const minBtn = screen.getByRole("button", { name: t("window.minimize") });
    minBtn.focus();
    fireEvent.click(minBtn);
    expect(document.activeElement).not.toBe(minBtn);
    fireEvent.click(screen.getByRole("button", { name: t("window.close") }));
    await vi.waitFor(() => {
      expect(invokeSpy).toHaveBeenCalledWith(
        "plugin:window|minimize",
        { label: "main" },
        undefined,
      );
      expect(invokeSpy).toHaveBeenCalledWith("plugin:window|close", { label: "main" }, undefined);
    });
  });
});
