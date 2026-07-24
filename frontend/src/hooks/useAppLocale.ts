import { useSyncExternalStore } from "react";
import {
  getAppLocale,
  subscribeAppLocale,
  type AppLocale,
} from "../i18n";

export function useAppLocale(): AppLocale {
  return useSyncExternalStore(subscribeAppLocale, getAppLocale, getAppLocale);
}
