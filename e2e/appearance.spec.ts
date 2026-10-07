import type { Locator, Page } from "@playwright/test";
import type { FixtureAppearance } from "./support/fixtures";
import { ALEX, DEFAULT_CONFIG } from "./support/fixtures";
import { expect, test } from "./support/test";
import { accountTrigger, text } from "./support/ui";

// The skin and cape sections of the account dialog, against the fake backend: no Minecraft
// service is ever contacted, and nothing here can change a real account.

const CAPE_A = "11111111-2222-4333-8444-555555555555";
const CAPE_B = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

const u32 = (n: number) =>
  Buffer.from([(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255]);
/** A file with a genuine PNG header of the given size (the fake only reads the header, like the UI). */
const png = (w = 64, h = 64) =>
  Buffer.concat([
    Buffer.from([
      0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52,
    ]),
    u32(w),
    u32(h),
    Buffer.from([8, 6, 0, 0, 0]),
  ]);
const file = (name: string, buffer: Buffer) => ({ name, mimeType: "image/png", buffer });

const look = (over: Partial<FixtureAppearance> = {}): Record<string, FixtureAppearance> => ({
  [ALEX.id]: { skinModel: "classic", capes: [], ...over },
});

const openAlex = async (page: Page): Promise<Locator> => {
  await accountTrigger(page).click();
  await page
    .getByRole("menuitem", { name: text("account.manage", { name: ALEX.username }) })
    .click();
  const dialog = page.getByRole("dialog", { name: ALEX.username });
  await expect(dialog).toBeVisible();
  return dialog;
};

const model = (dialog: Locator, which: "Classic" | "Slim") =>
  dialog.getByRole("radio", { name: which });
const chooseButton = (dialog: Locator) =>
  dialog.getByRole("button", { name: text("accountModal.choosePng") });
const fileInput = (dialog: Locator) => dialog.locator('input[type="file"]');

test.describe("skin", () => {
  test.describe("a Classic skin", () => {
    test.use({ backendConfig: { appearance: look() } });

    test("shows the current model, and choosing Slim then a PNG uploads it as Slim", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await expect(model(dialog, "Classic")).toBeChecked();
      await expect(dialog.locator("[data-skin-head]")).toHaveCount(0);

      await model(dialog, "Slim").click();
      await fileInput(dialog).setInputFiles(file("my-skin.png", png()));

      await expect
        .poll(async () =>
          (await backend.calls("set_account_skin")).map((c) => {
            const a = c.args as { id: string; variant: string; png: number[] };
            return { id: a.id, variant: a.variant, bytes: a.png.length };
          }),
        )
        .toEqual([{ id: ALEX.id, variant: "slim", bytes: png().length }]);
      // the account's new look is what the dialog and the avatar show, with no restart
      await expect(model(dialog, "Slim")).toBeChecked();
      await expect(dialog.locator("[data-skin-head]")).toHaveCount(1);
      // and so does the dock once the dialog is closed
      await page.keyboard.press("Escape");
      await expect(accountTrigger(page).locator("[data-skin-head]")).toHaveCount(1);
      // the file name is not kept anywhere in the page
      expect(await page.evaluate(() => document.body.innerHTML.includes("my-skin"))).toBe(false);
    });

    test("a file that is not a valid skin gets a plain sentence and changes nothing", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await fileInput(dialog).setInputFiles(
        file("notes.png", Buffer.from("this is not a png at all")),
      );
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.invalid_skin"));
      await fileInput(dialog).setInputFiles(file("big.png", png(128, 128)));
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.invalid_skin"));
      await expect(model(dialog, "Classic")).toBeChecked();
      await expect(dialog.locator("[data-skin-head]")).toHaveCount(0);
      expect(await backend.calls("reset_account_skin")).toHaveLength(0);
    });

    test("a file over the size limit is refused before anything is sent", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await fileInput(dialog).setInputFiles(
        file("huge.png", Buffer.concat([png(), Buffer.alloc(300 * 1024)])),
      );
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.invalid_skin"));
      expect(await backend.calls("set_account_skin")).toHaveLength(0);
    });

    test("while a change is running the controls are busy and a second one cannot start", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await backend.holdNext("set_account_skin");
      await fileInput(dialog).setInputFiles(file("a.png", png()));

      const busy = dialog.getByRole("button", { name: text("accountModal.uploading") });
      await expect(busy).toBeDisabled();
      await expect(
        dialog.getByRole("button", { name: text("accountModal.restoreDefault") }),
      ).toBeDisabled();
      await expect(model(dialog, "Slim")).toBeDisabled();
      // a second file chosen meanwhile starts nothing
      await fileInput(dialog).setInputFiles(file("b.png", png()));
      await backend.release();
      await expect(chooseButton(dialog)).toBeEnabled();
      expect(await backend.calls("set_account_skin")).toHaveLength(1);
    });

    test("the server refusing a change keeps the current look and says why", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await backend.failNext("set_account_skin", "rate_limited");
      await fileInput(dialog).setInputFiles(file("a.png", png()));
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.rate_limited"));
      await expect(model(dialog, "Classic")).toBeChecked();
      await expect(chooseButton(dialog)).toBeEnabled();
      await expect(dialog).toBeVisible();
    });
  });

  test.describe("a Slim skin", () => {
    test.use({ backendConfig: { appearance: look({ skinModel: "slim" }) } });

    const restore = (dialog: Locator) =>
      dialog.getByRole("button", { name: text("accountModal.restoreDefault") });
    const keep = (dialog: Locator) =>
      dialog.getByRole("button", { name: text("accountModal.keepSkin") });
    const question = (dialog: Locator) => dialog.getByText(text("accountModal.resetConfirm"));
    // A closing dialog loses `data-open` at once; wait two frames so a click has been handled.
    const stillOpen = async (page: Page, dialog: Locator) => {
      await page.evaluate(
        () =>
          new Promise<void>((done) =>
            requestAnimationFrame(() => requestAnimationFrame(() => done())),
          ),
      );
      await expect(dialog).toHaveAttribute("data-open", "");
      await expect(dialog).not.toHaveAttribute("data-closed", "");
    };

    test("shows Slim, and Restore default asks first, then goes back to the default skin", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await expect(model(dialog, "Slim")).toBeChecked();

      await restore(dialog).click();
      await expect(question(dialog)).toBeVisible();
      expect(await backend.calls("reset_account_skin")).toHaveLength(0);

      // only the explicit confirmation changes the account
      await restore(dialog).last().click();
      await expect
        .poll(async () => (await backend.calls("reset_account_skin")).map((c) => c.args))
        .toEqual([{ id: ALEX.id }]);
      await expect(question(dialog)).toHaveCount(0);
      // no model is reported for a default skin: the choice stays where it was, nothing is invented
      await expect(chooseButton(dialog)).toBeEnabled();
      await expect(dialog.locator("[data-skin-head]")).toHaveCount(0);
    });

    test("Keep current skin, Escape and the backdrop all leave the skin alone", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await restore(dialog).click();
      await expect(question(dialog)).toBeVisible();

      // Escape and a click on the backdrop do not dismiss the question or the dialog
      await page.keyboard.press("Escape");
      await stillOpen(page, dialog);
      await expect(question(dialog)).toBeVisible();
      await page.mouse.click(8, 8);
      await stillOpen(page, dialog);
      await expect(question(dialog)).toBeVisible();

      await keep(dialog).click();
      await expect(question(dialog)).toHaveCount(0);
      await expect(model(dialog, "Slim")).toBeChecked();
      expect(await backend.calls("reset_account_skin")).toHaveLength(0);
      // with the question answered, Escape closes the dialog as usual
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
    });

    test("while the reset runs nothing else can start and it is sent once", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await restore(dialog).click();
      await backend.holdNext("reset_account_skin");
      await restore(dialog).last().click();
      await expect(
        dialog.getByRole("button", { name: text("accountModal.restoring") }),
      ).toBeDisabled();
      await expect(keep(dialog)).toBeDisabled();
      await expect(chooseButton(dialog)).toBeDisabled();
      await backend.release();
      await expect(chooseButton(dialog)).toBeEnabled();
      expect(await backend.calls("reset_account_skin")).toHaveLength(1);
    });

    test("a refused reset keeps the Slim skin and says why", async ({ page, backend, openApp }) => {
      await openApp();
      const dialog = await openAlex(page);
      await restore(dialog).click();
      await backend.failNext("reset_account_skin", "rate_limited");
      await restore(dialog).last().click();
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.rate_limited"));
      await expect(question(dialog)).toHaveCount(0);
      await expect(model(dialog, "Slim")).toBeChecked();
      await expect(restore(dialog)).toBeEnabled();
    });
  });

  test.describe("an account that must sign in again", () => {
    test.use({
      backendConfig: { accounts: [{ ...ALEX, needsSignIn: true }], appearance: look() },
    });

    test("says so in words, offers no change, and asks the service nothing", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await expect(dialog.getByRole("alert")).toHaveText(text("appearance.needsSignIn"));
      await expect(chooseButton(dialog)).toBeDisabled();
      await expect(dialog.getByRole("button", { name: text("common.signInAgain") })).toBeVisible();
      expect(await backend.calls("account_appearance")).toHaveLength(0);
    });
  });

  test.describe("locked accounts", () => {
    test.use({ backendConfig: { appearance: look() } });

    test("explain themselves and unlock right there, then load the look", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      await backend.failNext("account_appearance", "vault_locked");
      const dialog = await openAlex(page);
      await expect(dialog.getByRole("alert").first()).toHaveText(
        text("error.appearance.vault_locked"),
      );
      await expect(chooseButton(dialog)).toBeDisabled();
      await dialog.getByLabel(text("common.passphrase")).fill("fake test passphrase");
      await dialog.getByRole("button", { name: text("common.unlock") }).click();
      await expect(chooseButton(dialog)).toBeEnabled();
      await expect(dialog.getByLabel(text("common.passphrase"))).toHaveCount(0);
      expect(await backend.calls("unlock_vault")).toHaveLength(1);
    });
  });
});

test.describe("capes", () => {
  test.describe("none owned", () => {
    test.use({ backendConfig: { appearance: look() } });

    test("show a short empty state", async ({ page, openApp }) => {
      await openApp();
      const dialog = await openAlex(page);
      await expect(dialog.getByText(text("appearance.noCapes"))).toBeVisible();
      await expect(dialog.getByRole("button", { name: text("appearance.hideCape") })).toHaveCount(
        0,
      );
    });
  });

  test.describe("owned", () => {
    test.use({
      backendConfig: {
        appearance: look({
          capes: [
            { id: CAPE_A, alias: "Migrator", active: true },
            { id: CAPE_B, alias: null, active: false },
          ],
        }),
      },
    });

    test("are listed by name with the active one marked and no id shown", async ({
      page,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      const rows = dialog.getByRole("listitem");
      await expect(rows).toHaveCount(2);
      await expect(rows.nth(0)).toContainText("Migrator");
      await expect(rows.nth(0)).toContainText(text("appearance.equipped"));
      await expect(rows.nth(1)).toContainText(text("appearance.capeUnnamed", { n: 2 }));
      await expect(
        dialog.getByRole("button", { name: text("appearance.equipNamed", { name: "Migrator" }) }),
      ).toHaveCount(0);
      expect(await dialog.innerText()).not.toContain(CAPE_A);
      expect(await dialog.innerText()).not.toContain(CAPE_B);
    });

    test("equipping one sends that cape for that account and moves the mark", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await dialog
        .getByRole("button", {
          name: text("appearance.equipNamed", { name: text("appearance.capeUnnamed", { n: 2 }) }),
        })
        .click();
      await expect
        .poll(async () => (await backend.calls("set_account_cape")).map((c) => c.args))
        .toEqual([{ id: ALEX.id, capeId: CAPE_B }]);
      const rows = dialog.getByRole("listitem");
      await expect(rows.nth(1)).toContainText(text("appearance.equipped"));
      await expect(rows.nth(0)).not.toContainText(text("appearance.equipped"));
    });

    test("hiding the cape sends no id, and nothing is marked afterwards", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await dialog.getByRole("button", { name: text("appearance.hideCape") }).click();
      await expect
        .poll(async () => (await backend.calls("set_account_cape")).map((c) => c.args))
        .toEqual([{ id: ALEX.id, capeId: null }]);
      await expect(dialog.getByText(text("appearance.equipped"))).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: text("appearance.hideCape") })).toHaveCount(
        0,
      );
    });

    test("a refusal from the server leaves the capes exactly as they were", async ({
      page,
      backend,
      openApp,
    }) => {
      await openApp();
      const dialog = await openAlex(page);
      await backend.failNext("set_account_cape", "cape_not_owned");
      await dialog
        .getByRole("button", {
          name: text("appearance.equipNamed", { name: text("appearance.capeUnnamed", { n: 2 }) }),
        })
        .click();
      await expect(dialog.getByRole("alert")).toHaveText(text("error.appearance.cape_not_owned"));
      const rows = dialog.getByRole("listitem");
      await expect(rows.nth(0)).toContainText(text("appearance.equipped"));
      await expect(rows.nth(1)).not.toContainText(text("appearance.equipped"));
    });
  });
});

test.describe("a long cape list", () => {
  const names = Array.from({ length: 14 }, (_, i) => `Cape name ${i + 1}`);
  const capes = names.map((alias, i) => ({
    id: `${(0x10000000 + i).toString(16)}-2222-4333-8444-555555555555`,
    alias,
    active: i === 11,
  }));
  test.use({ backendConfig: { appearance: look({ capes }) } });

  test("starts with the equipped cape in view, and Hide cape is always in reach", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openAlex(page);
    const rows = dialog.getByRole("listitem");
    await expect(rows).toHaveCount(14);
    // the 12th of 14 is equipped: the list was scrolled inside itself to show it
    await expect(rows.nth(11)).toContainText(text("appearance.equipped"));
    // ... which is checked against the list's own box, not the window (the dialog scrolls too)
    const inList = await rows.nth(11).evaluate((li) => {
      const row = li.getBoundingClientRect();
      const list = (li.parentElement as HTMLElement).getBoundingClientRect();
      return row.top >= list.top - 1 && row.bottom <= list.bottom + 1;
    });
    expect(inList).toBe(true);
    await expect(dialog.getByRole("button", { name: text("appearance.hideCape") })).toBeVisible();
  });

  test("shows at least five rows, with a fixed footer under them that never covers one", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openAlex(page);
    await expect(dialog.getByRole("listitem")).toHaveCount(14);
    const m = await dialog.evaluate((d) => {
      const list = d.querySelector("ul") as HTMLElement;
      const l = list.getBoundingClientRect();
      const footer = (list.nextElementSibling as HTMLElement).getBoundingClientRect();
      const rows = [...list.children].map((li) => li.getBoundingClientRect());
      const whole = rows.filter((r) => r.top >= l.top - 1 && r.bottom <= l.bottom + 1).length;
      return {
        wholeRows: whole,
        footerTop: footer.top,
        listBottom: l.bottom,
        listScrolls: list.scrollHeight > list.clientHeight,
        // the footer is not part of the scrolling rows
        footerInsideList: list.contains(list.nextElementSibling),
      };
    });
    expect(m.wholeRows).toBeGreaterThanOrEqual(5);
    expect(m.listScrolls).toBe(true);
    expect(m.footerInsideList).toBe(false);
    expect(m.footerTop).toBeGreaterThanOrEqual(m.listBottom - 0.5);
  });

  test("the dialog scrolls to the footer, and the rows scroll on their own", async ({
    page,
    openApp,
  }) => {
    await openApp();
    const dialog = await openAlex(page);
    const hide = dialog.getByRole("button", { name: text("appearance.hideCape") });
    // reaching the footer is the dialog's own scroll...
    await hide.scrollIntoViewIfNeeded();
    await expect(hide).toBeInViewport({ ratio: 1 });
    expect(await dialog.evaluate((d) => d.scrollTop)).toBeGreaterThan(0);
    // ...and scrolling the rows moves only the rows
    const before = await dialog.evaluate((d) => d.scrollTop);
    const rows = dialog.locator("ul");
    await rows.evaluate((ul) => (ul.scrollTop = ul.scrollHeight));
    expect(await rows.evaluate((ul) => ul.scrollTop)).toBeGreaterThan(0);
    expect(await dialog.evaluate((d) => d.scrollTop)).toBe(before);
    await expect(hide).toBeVisible();
  });
});

for (const language of ["en", "ru"] as const) {
  test.describe(`the skin controls in ${language}`, () => {
    test.use({
      backendConfig: { appearance: look(), ui: { ...DEFAULT_CONFIG.ui, language } },
    });

    test("are two rows: the model and upload, then an outlined Restore default on its own, aligned", async ({
      page,
      openApp,
    }) => {
      await openApp();
      await accountTrigger(page).click();
      await page.getByRole("menuitem").first().click();
      const dialog = page.getByRole("dialog");
      await expect(dialog).toBeVisible();
      const m = await dialog.evaluate((d) => {
        const seg = d.querySelector('[role="radiogroup"]') as HTMLElement;
        const buttons = [...d.querySelectorAll("button")];
        const choose = buttons.find((b) => /PNG/.test(b.textContent ?? "")) as HTMLElement;
        const restore = buttons.find((b) =>
          /Restore default|Вернуть стандартный/.test(b.textContent ?? ""),
        ) as HTMLElement;
        const s = seg.getBoundingClientRect();
        const c = choose.getBoundingClientRect();
        const r = restore.getBoundingClientRect();
        return {
          // first row: the model and the upload side by side
          sameFirstRow:
            Math.abs(s.top + s.height / 2 - (c.top + c.height / 2)) < 4 && c.left > s.right,
          // second row: below the first, never beside it
          below: r.top >= Math.max(s.bottom, c.bottom) - 1,
          // its outline starts where the controls above start
          left: r.left,
          segLeft: s.left,
          // and it looks like a button: a visible outline, not bare text
          outlined: getComputedStyle(restore).boxShadow !== "none",
        };
      });
      expect(m.sameFirstRow).toBe(true);
      expect(m.below).toBe(true);
      expect(Math.abs(m.left - m.segLeft)).toBeLessThanOrEqual(1);
      expect(m.outlined).toBe(true);
    });
  });
}
