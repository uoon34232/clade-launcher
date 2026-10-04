import { expect, test } from "@playwright/test";

for (const [width, height] of [
  [1280, 720],
  [1440, 900],
  [1920, 1080],
]) {
  test(`home has no overflow at ${width}x${height}`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    const overflow = await page.evaluate(() => {
      const d = document.documentElement;
      return d.scrollWidth > innerWidth || d.scrollHeight > innerHeight;
    });
    expect(overflow).toBe(false);
  });
}
