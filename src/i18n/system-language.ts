import { DEFAULT_LANGUAGE, SUPPORTED_LANGUAGES } from "./languages";
import type { SupportedLanguage } from "./languages";

/**
 * Map an arbitrary BCP-47-ish tag (e.g. "zh-CN", "en_US", "ja") to one of the
 * supported language codes, or null when nothing matches.
 *
 * Matching rules, applied in order:
 *   1. exact code match, case-insensitive, "_" treated as "-"
 *   2. primary subtag match ("zh-CN" -> "zh", "en-US" -> "en")
 */
export function normalizeLanguage(
  tag: string | null | undefined,
): SupportedLanguage | null {
  if (!tag) return null;
  const lower = tag.toLowerCase().replace(/_/g, "-");
  const exact = SUPPORTED_LANGUAGES.find((l) => l.code.toLowerCase() === lower);
  if (exact) return exact.code;
  const primary = lower.split("-")[0];
  const match = SUPPORTED_LANGUAGES.find(
    (l) => l.code.toLowerCase() === primary,
  );
  return match ? match.code : null;
}

/**
 * Ordered locale tags reported by the ambient browser/WebView, most preferred
 * first. Returns an empty list outside a browser environment.
 */
export function browserLanguagePreferences(): string[] {
  if (typeof navigator === "undefined") return [];
  const list =
    navigator.languages && navigator.languages.length > 0
      ? [...navigator.languages]
      : [];
  if (navigator.language && !list.includes(navigator.language)) {
    list.unshift(navigator.language);
  }
  return list;
}

/**
 * Pick the best supported language for the given ordered locale preferences.
 * Pure and injectable so it can be unit-tested and reused outside a browser.
 *
 * @param preferred Ordered locale tags, most preferred first. Callers can pass
 *   an OS-level locale ahead of the browser list; when omitted the ambient
 *   browser preferences are used. Falls back to DEFAULT_LANGUAGE when nothing
 *   matches or when no preferences are available (e.g. non-browser runtime).
 */
export function detectSystemLanguage(
  preferred?: readonly string[],
): SupportedLanguage {
  const candidates = preferred ?? browserLanguagePreferences();
  for (const tag of candidates) {
    const match = normalizeLanguage(tag);
    if (match) return match;
  }
  return DEFAULT_LANGUAGE;
}
