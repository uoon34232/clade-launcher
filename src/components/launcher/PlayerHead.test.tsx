import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { parseAccounts } from "@/lib/accounts/api";
import { testSkin } from "@/test/testSkin";
import { PlayerHead } from "./PlayerHead";

afterEach(cleanup);

const layers = (c: HTMLElement) =>
  [...c.querySelectorAll<SVGSVGElement>("svg[data-skin-head] > svg")].map((s) => ({
    viewBox: s.getAttribute("viewBox"),
    href: s.querySelector("image")?.getAttribute("href"),
    layer: s.querySelector("image")?.getAttribute("data-layer"),
  }));

describe("PlayerHead", () => {
  it("cuts the face and then the hat layer out of the current skin, nearest-neighbour", () => {
    const skin = testSkin(64, 64);
    const { container } = render(<PlayerHead account={{ username: "Alex", skin }} />);
    expect(layers(container)).toEqual([
      { viewBox: "8 8 8 8", href: skin, layer: "face" },
      { viewBox: "40 8 8 8", href: skin, layer: "hat" }, // drawn after the face: over it
    ]);
    const images = container.querySelectorAll("image");
    expect(images).toHaveLength(2);
    for (const image of images) {
      expect(image.getAttribute("width")).toBe("64");
      expect(image.getAttribute("height")).toBe("64");
      expect(image.getAttribute("preserveAspectRatio")).toBe("none");
      expect(image.getAttribute("image-rendering")).toBe("pixelated");
      expect(image.getAttribute("style")).toContain("image-rendering: pixelated");
    }
    expect(container.querySelector("svg")?.getAttribute("shape-rendering")).toBe("crispEdges");
  });

  it("uses the real texture height for the old 64x32 format and scales high-resolution skins", () => {
    const old = render(<PlayerHead account={{ username: "Old", skin: testSkin(64, 32) }} />);
    expect(old.container.querySelector("image")?.getAttribute("height")).toBe("32");
    expect(layers(old.container)[0].viewBox).toBe("8 8 8 8");
    cleanup();
    const hd = render(<PlayerHead account={{ username: "Hd", skin: testSkin(128, 128) }} />);
    expect(layers(hd.container).map((l) => l.viewBox)).toEqual(["16 16 16 16", "80 16 16 16"]);
    expect(hd.container.querySelector("image")?.getAttribute("width")).toBe("128");
  });

  it("draws the neutral face when there is no skin, an unusable one, or the image fails to load", () => {
    const placeholder = (c: HTMLElement) => c.querySelectorAll("svg rect").length === 64;
    const none = render(<PlayerHead account={{ username: "A", skin: null }} />);
    expect(placeholder(none.container)).toBe(true);
    expect(none.container.querySelector("image")).toBeNull();
    cleanup();
    expect(placeholder(render(<PlayerHead />).container)).toBe(true);
    cleanup();
    const bad = render(<PlayerHead account={{ username: "A", skin: testSkin(33, 7) }} />);
    expect(placeholder(bad.container)).toBe(true);
    cleanup();

    const skin = testSkin(64, 64, 1);
    const view = render(<PlayerHead account={{ username: "A", skin }} />);
    fireEvent.error(view.container.querySelector("image[data-layer=face]")!);
    expect(placeholder(view.container)).toBe(true);
    // A later, different skin gets its own chance.
    view.rerender(<PlayerHead account={{ username: "A", skin: testSkin(64, 64, 2) }} />);
    expect(view.container.querySelector("image")).not.toBeNull();
  });

  it("uses pre-parsed skinDimensions from Account without re-parsing the PNG header on render", () => {
    const parsed = parseAccounts({
      accounts: [{ id: "a", username: "Alex", uuid: "u", skin: testSkin(128, 128) }],
      active: "a",
      capabilities: {},
    });
    expect(parsed.accounts[0].skinDimensions).toEqual({ width: 128, height: 128 });

    // Pre-parsed skinDimensions is read directly without running skinSize(skin) on render.
    const view = render(
      <PlayerHead
        account={{
          username: "Alex",
          skin: "data:image/png;base64,pre-parsed",
          skinDimensions: { width: 128, height: 128 },
        }}
      />,
    );
    expect(layers(view.container).map((l) => l.viewBox)).toEqual(["16 16 16 16", "80 16 16 16"]);
    expect(view.container.querySelector("image")?.getAttribute("width")).toBe("128");
  });

  it("renders the placeholder without calling skinSize when skinDimensions is explicitly null", () => {
    const view = render(
      <PlayerHead
        account={{
          username: "Alex",
          skin: testSkin(64, 64),
          skinDimensions: null,
        }}
      />,
    );
    expect(view.container.querySelector("svg[data-skin-head]")).toBeNull();
    expect(view.container.querySelector("image")).toBeNull();
    expect(view.container.querySelectorAll("svg rect")).toHaveLength(64);
  });

  it("forwards className to both the rendered skin head and the placeholder avatar", () => {
    const rendered = render(
      <PlayerHead
        account={{ username: "Alex", skin: testSkin(64, 64) }}
        className="size-8 rounded"
      />,
    );
    const headSvg = rendered.container.querySelector("svg[data-skin-head]");
    expect(headSvg?.getAttribute("class")).toBe("size-8 rounded [image-rendering:pixelated]");
    cleanup();

    const placeholderView = render(
      <PlayerHead account={{ username: "Alex", skin: null }} className="size-8 rounded" />,
    );
    const avatarSvg = placeholderView.container.querySelector("svg");
    expect(placeholderView.container.querySelector("svg[data-skin-head]")).toBeNull();
    expect(avatarSvg?.getAttribute("class")).toBe("size-8 rounded");
  });
});
