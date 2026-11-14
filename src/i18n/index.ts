import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import {
  DEFAULT_LANGUAGE,
  SUPPORTED_LANGUAGES,
  TRANSLATION_RESOURCES,
  type SupportedLanguage,
} from "./languages";
import {
  browserLanguagePreferences,
  detectSystemLanguage,
  normalizeLanguage,
} from "./system-language";

export type { SupportedLanguage };
export { DEFAULT_LANGUAGE, SUPPORTED_LANGUAGES };

/**
 * Resolve the language for the very first paint, before React mounts.
 *
 * i18next initialises while `main.tsx` is still evaluating its imports, so the
 * `lng` passed here is what the first frame renders. This used to be
 * hard-coded to `DEFAULT_LANGUAGE` ("en"), and the real language was only
 * applied later inside `useBootstrap` — which waits on six sequential IPC
 * calls. The result was an English UI for a few hundred milliseconds on every
 * launch before it flipped to Chinese.
 *
 * The persisted zustand value is authoritative when present: it is exactly
 * what the user last selected. On a first run it is absent, so fall back to
 * the ambient WebView locale — reading `navigator.languages` is synchronous,
 * so even that path paints the right language immediately.
 */
function initialLanguage(): SupportedLanguage {
  try {
    const raw = localStorage.getItem("weave.ui");
    if (raw) {
      const parsed = JSON.parse(raw) as { state?: { language?: unknown } };
      const persisted = normalizeLanguage(
        typeof parsed.state?.language === "string"
          ? parsed.state.language
          : null,
      );
      if (persisted) return persisted;
    }
  } catch {
    // Corrupt or unavailable storage: fall through to locale detection.
  }
  return detectSystemLanguage(browserLanguagePreferences());
}

/**
 * The language chosen for the first render. Exported so `main.tsx` can mirror
 * it onto `<html lang>` before the first paint.
 */
export const INITIAL_LANGUAGE = initialLanguage();

// Initialize i18next
void i18n.use(initReactI18next).init({
  resources: TRANSLATION_RESOURCES,
  lng: INITIAL_LANGUAGE,
  fallbackLng: "en",
  interpolation: {
    escapeValue: false,
  },
  returnNull: false,
  returnEmptyString: false,
});

export default i18n;
