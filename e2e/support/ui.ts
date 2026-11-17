// Small helpers for finding the launcher's controls by what a person sees.

import type { Locator, Page } from "@playwright/test";
import { en } from "@/locales/en";
import { ru } from "@/locales/ru";

type EnKey = keyof typeof en;

/** English catalog text with `{params}` filled in (the suite runs in English unless a test switches). */
export function text(key: EnKey, params: Record<string, string | number> = {}): string {
  const raw = en[key];
  if (typeof raw !== "string") throw new Error(`"${key}" is not a plain message`);
  return raw.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? `{${k}}`));
}

/** Russian catalog text with `{params}` filled in. */
export function textRu(key: keyof typeof ru, params: Record<string, string | number> = {}): string {
  const raw = ru[key];
  if (typeof raw !== "string") throw new Error(`"${String(key)}" is not a plain message`);
  return raw.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? `{${k}}`));
}

export const dock = (page: Page) => page.getByRole("navigation", { name: "Clade" });
export const playButton = (page: Page) => dock(page).locator("button[data-state]");
export const accountTrigger = (page: Page) =>
  dock(page).getByRole("button", { name: /^(Account:|Add an account|Аккаунт:|Добавить аккаунт)/ });
export const profileTrigger = (page: Page) =>
  dock(page).getByRole("button", { name: /^(Profile:|Create a profile|Профиль:|Создать профиль)/ });
export const settingsButton = (page: Page) =>
  dock(page).getByRole("button", { name: /^(Settings|Настройки)$/ });
export const headline = (page: Page) => page.getByRole("heading", { level: 1 });

/** Resolves once the headline has finished its fade (no animation left on it). */
export async function headlineSettled(page: Page): Promise<void> {
  await page.waitForFunction(
    () => (document.querySelector("h1")?.getAnimations({ subtree: true }).length ?? 1) === 0,
  );
}

/** The currently focused element. */
export const focused = (page: Page): Locator => page.locator(":focus");
