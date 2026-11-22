import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setLocale, t } from "@/lib/i18n";
import { WindowChrome } from "./WindowChrome";

const invokeSpy = vi.fn<(cmd: string, args?: unknown) => Promise<void>>(() => Promise.resolve());

beforeEach(() => {
  setLocale("en");
  invokeSpy.mockReset();
  invokeSpy.mockResolvedValue(undefined);
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
    const unframedStrip = container.firstElementChild as HTMLElement;
    expect(unframedStrip.className).toContain("border-transparent");
    expect(screen.getByRole("button", { name: t("window.close") }).className).not.toContain(
      "chrome-closing-pulse",
    );

    rerender(<WindowChrome framed closing maximized={false} onToggleMaximize={onToggleMaximize} />);
    const strip = container.firstElementChild as HTMLElement;
    expect(strip.className).toContain("h-[44px]");
    expect(strip.className).toContain("border-graphite/10");
    expect(strip.getAttribute("data-tauri-drag-region")).toBe("true");
    expect(screen.getByRole("button", { name: t("window.close") }).className).toContain(
      "chrome-closing-pulse",
    );

    const maxBtn = screen.getByRole("button", { name: t("window.maximize") });
    maxBtn.focus();
    expect(document.activeElement).toBe(maxBtn);
    fireEvent.click(maxBtn, { detail: 1 });
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);
    expect(document.activeElement).not.toBe(maxBtn);

    fireEvent.mouseDown(strip, { button: 0, detail: 2 });
    expect(onToggleMaximize).toHaveBeenCalledTimes(2);

    rerender(<WindowChrome framed closing={false} maximized onToggleMaximize={onToggleMaximize} />);
    expect(strip.hasAttribute("data-tauri-drag-region")).toBe(false);
    expect(screen.getByRole("button", { name: t("window.close") }).className).not.toContain(
      "chrome-closing-pulse",
    );
    const restoreBtn = screen.getByRole("button", { name: t("window.restore") });
    restoreBtn.focus();
    fireEvent.click(restoreBtn, { detail: 1 });
    expect(onToggleMaximize).toHaveBeenCalledTimes(3);
    expect(document.activeElement).not.toBe(restoreBtn);

    const minBtn = screen.getByRole("button", { name: t("window.minimize") });
    minBtn.focus();
    fireEvent.click(minBtn, { detail: 1 });
    expect(document.activeElement).not.toBe(minBtn);
    const closeBtn = screen.getByRole("button", { name: t("window.close") });
    closeBtn.focus();
    fireEvent.click(closeBtn, { detail: 1 });
    expect(document.activeElement).not.toBe(closeBtn);
    await vi.waitFor(() => {
      expect(invokeSpy).toHaveBeenCalledWith(
        "plugin:window|minimize",
        { label: "main" },
        undefined,
      );
      expect(invokeSpy).toHaveBeenCalledWith("plugin:window|close", { label: "main" }, undefined);
    });
  });

  it("preserves keyboard focus and blurs on pointer click in unframed and framed modes", async () => {
    const onToggleMaximize = vi.fn();
    const { rerender } = render(
      <WindowChrome framed={false} maximized={false} onToggleMaximize={onToggleMaximize} />,
    );

    const unframedMin = screen.getByRole("button", { name: t("window.minimize") });
    unframedMin.focus();
    expect(document.activeElement).toBe(unframedMin);
    fireEvent.click(unframedMin, { detail: 0 });
    expect(document.activeElement).toBe(unframedMin);
    fireEvent.click(unframedMin, { detail: 1 });
    expect(document.activeElement).not.toBe(unframedMin);

    const unframedClose = screen.getByRole("button", { name: t("window.close") });
    unframedClose.focus();
    expect(document.activeElement).toBe(unframedClose);
    fireEvent.click(unframedClose, { detail: 0 });
    expect(document.activeElement).toBe(unframedClose);
    fireEvent.click(unframedClose, { detail: 1 });
    expect(document.activeElement).not.toBe(unframedClose);

    rerender(<WindowChrome framed maximized={false} onToggleMaximize={onToggleMaximize} />);

    const framedMax = screen.getByRole("button", { name: t("window.maximize") });
    framedMax.focus();
    fireEvent.click(framedMax, { detail: 0 });
    expect(document.activeElement).toBe(framedMax);
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);

    const framedMin = screen.getByRole("button", { name: t("window.minimize") });
    framedMin.focus();
    fireEvent.click(framedMin, { detail: 0 });
    expect(document.activeElement).toBe(framedMin);

    const framedClose = screen.getByRole("button", { name: t("window.close") });
    framedClose.focus();
    fireEvent.click(framedClose, { detail: 0 });
    expect(document.activeElement).toBe(framedClose);

    await vi.waitFor(() => {
      expect(invokeSpy).toHaveBeenCalledWith(
        "plugin:window|minimize",
        { label: "main" },
        undefined,
      );
      expect(invokeSpy).toHaveBeenCalledWith("plugin:window|close", { label: "main" }, undefined);
    });
  });

  it("guards drag-strip double-clicks across framed state, child buttons, and click types", () => {
    const onToggleMaximize = vi.fn();
    const parentMouseDown = vi.fn();
    const dispatchMouseDown = (target: Element, init: MouseEventInit) => {
      const event = new MouseEvent("mousedown", { bubbles: true, cancelable: true, ...init });
      let bubbleCancelledDuringDispatch = false;
      const probe = (e: Event) => {
        bubbleCancelledDuringDispatch = e.cancelBubble;
      };
      strip.addEventListener("mousedown", probe);
      fireEvent(target, event);
      strip.removeEventListener("mousedown", probe);
      return { event, bubbleCancelledDuringDispatch };
    };

    const { container, rerender } = render(
      <div onMouseDown={parentMouseDown}>
        <WindowChrome framed={false} maximized={false} onToggleMaximize={onToggleMaximize} />
      </div>,
    );
    const wrapper = container.firstElementChild as HTMLElement;
    const strip = wrapper.firstElementChild as HTMLElement;

    const unframedDouble = dispatchMouseDown(strip, { button: 0, detail: 2 });
    expect(unframedDouble.event.defaultPrevented).toBe(true);
    expect(unframedDouble.bubbleCancelledDuringDispatch).toBe(true);
    expect(parentMouseDown).not.toHaveBeenCalled();
    expect(onToggleMaximize).not.toHaveBeenCalled();

    rerender(
      <div onMouseDown={parentMouseDown}>
        <WindowChrome framed maximized={false} onToggleMaximize={onToggleMaximize} />
      </div>,
    );

    const framedStripDouble = dispatchMouseDown(strip, { button: 0, detail: 2 });
    expect(framedStripDouble.event.defaultPrevented).toBe(true);
    expect(framedStripDouble.bubbleCancelledDuringDispatch).toBe(true);
    expect(parentMouseDown).not.toHaveBeenCalled();
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);

    const wordmarkBtn = screen.getByRole("button", { name: t("window.wordmark") });
    const wordmarkDouble = dispatchMouseDown(wordmarkBtn, { button: 0, detail: 2 });
    expect(wordmarkDouble.event.defaultPrevented).toBe(true);
    expect(wordmarkDouble.bubbleCancelledDuringDispatch).toBe(true);
    expect(parentMouseDown).not.toHaveBeenCalled();
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);

    const minBtn = screen.getByRole("button", { name: t("window.minimize") });
    const controlDouble = dispatchMouseDown(minBtn, { button: 0, detail: 2 });
    expect(controlDouble.event.defaultPrevented).toBe(true);
    expect(controlDouble.bubbleCancelledDuringDispatch).toBe(true);
    expect(parentMouseDown).not.toHaveBeenCalled();
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);

    const singlePrimary = dispatchMouseDown(strip, { button: 0, detail: 1 });
    expect(singlePrimary.event.defaultPrevented).toBe(false);
    expect(singlePrimary.bubbleCancelledDuringDispatch).toBe(false);
    expect(parentMouseDown).toHaveBeenCalledTimes(1);
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);

    const nonPrimaryDouble = dispatchMouseDown(strip, { button: 2, detail: 2 });
    expect(nonPrimaryDouble.event.defaultPrevented).toBe(false);
    expect(nonPrimaryDouble.bubbleCancelledDuringDispatch).toBe(false);
    expect(parentMouseDown).toHaveBeenCalledTimes(2);
    expect(onToggleMaximize).toHaveBeenCalledTimes(1);
  });

  it("renders nothing and skips Tauri window APIs outside Tauri", () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;

    const { container } = render(<WindowChrome framed maximized={false} />);

    expect(container.firstElementChild).toBeNull();
    expect(screen.queryByRole("button")).toBeNull();
    expect(invokeSpy).not.toHaveBeenCalled();
  });

  it("swallows rejected Tauri window IPC calls without unhandled rejections", async () => {
    invokeSpy.mockRejectedValue(new Error("ipc denied"));
    render(<WindowChrome framed={false} maximized={false} />);

    fireEvent.click(screen.getByRole("button", { name: t("window.minimize") }), { detail: 1 });
    fireEvent.click(screen.getByRole("button", { name: t("window.close") }), { detail: 1 });

    await vi.waitFor(() => {
      expect(invokeSpy).toHaveBeenCalledWith(
        "plugin:window|minimize",
        { label: "main" },
        undefined,
      );
      expect(invokeSpy).toHaveBeenCalledWith("plugin:window|close", { label: "main" }, undefined);
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
});
