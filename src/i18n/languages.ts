import type { Resource } from "i18next";

import en from "./locales/en";
import ru from "./locales/ru";
import zh from "./locales/zh";

/**
 * Single source of truth for the languages the UI ships with.
 *
 * Adding a language only requires:
 *   1. dropping a locale file under ./locales
 *   2. registering it in LOCALES below
 *   3. adding the same code/label to the backend (src-tauri/src/services/i18n.rs)
 * Everything else (language picker, system-language detection, i18next
 * resources) is derived from this list, so nothing else needs to change.
 */
export const LOCALES = [
  { code: "en", label: "English", resource: en },
  { code: "ru", label: "Русский", resource: ru },
  { code: "zh", label: "简体中文", resource: zh },
] as const;

export type SupportedLanguage = (typeof LOCALES)[number]["code"];

export const DEFAULT_LANGUAGE: SupportedLanguage = "en";

/** Language codes paired with their display label, for UI pickers. */
export const SUPPORTED_LANGUAGES: { code: SupportedLanguage; label: string }[] =
  LOCALES.map(({ code, label }) => ({ code, label }));

/** i18next resource bundle built from LOCALES. */
export const TRANSLATION_RESOURCES: Record<string, { translation: Resource }> =
  Object.fromEntries(
    LOCALES.map(({ code, resource }) => [code, { translation: resource }]),
  );
