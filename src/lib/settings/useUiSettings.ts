import { useSyncExternalStore } from "react";
import { getUiSettings, subscribeUiSettings, type UiSettings } from "./uiSettings";

/** The launcher's settings, kept current when they change here or in the backend. */
export function useUiSettings(): UiSettings {
  return useSyncExternalStore(subscribeUiSettings, getUiSettings, getUiSettings);
}
