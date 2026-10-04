// Turns the backend's structured failures (a stable `code` plus named `params`) into text in the
// current language. Rust never sends finished prose for the user; its English `message` is for
// logs and is never shown. A code with no wording falls back to the domain's general message, so
// an unexpected failure still reads as a plain sentence.

import { formatBytes } from "@/features/launcher/profileUi";
import { hasMessage, tDynamic, type Params } from "./index";

/** Which part of the launcher an error came from; the same code can mean different things. */
export type ErrorDomain =
  "launch" | "prepare" | "signIn" | "vault" | "account" | "profile" | "catalog";

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

/** The stable code carried by a backend failure, if it has one. */
export function errorCode(e: unknown): string | null {
  return isObj(e) && typeof e.code === "string" ? e.code : null;
}

function errorParams(e: unknown): Record<string, unknown> {
  return isObj(e) && isObj(e.params) ? e.params : {};
}

/** Parameters the template can use: strings and numbers only, byte counts turned into sizes. */
function templateParams(domain: ErrorDomain, raw: Record<string, unknown>): Params {
  const out: Record<string, string | number> = {};
  for (const [k, v] of Object.entries(raw)) {
    if (typeof v === "string" || typeof v === "number") out[k] = v;
  }
  if (domain === "prepare") {
    if (typeof raw.requiredBytes === "number") out.required = formatBytes(raw.requiredBytes);
    if (typeof raw.availableBytes === "number") out.available = formatBytes(raw.availableBytes);
  }
  return out;
}

/** The user-facing sentence for a failure code in a domain. */
export function describeCode(
  domain: ErrorDomain,
  code: string | null,
  rawParams: Record<string, unknown> = {},
): string {
  let c = code;
  // One sign-in code has two wordings: the account owns Minecraft or it does not.
  if (domain === "signIn" && c === "no_java_profile" && rawParams.ownsMinecraft === true)
    c = "no_java_profile_owned";
  const params = templateParams(domain, rawParams);
  const candidates = [
    c && `error.${domain}.${c}`,
    c && `error.common.${c}`,
    `error.${domain}.unknown`,
    "error.generic",
  ];
  const key = candidates.find((k): k is string => !!k && hasMessage(k)) ?? "error.generic";
  return tDynamic(key, params);
}

/** The user-facing sentence for whatever was thrown. Never shows the backend's English detail. */
export function describeError(domain: ErrorDomain, e: unknown): string {
  return describeCode(domain, errorCode(e), errorParams(e));
}
