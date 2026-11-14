import { describe, it, expect, beforeEach, afterEach } from "vitest";

import { DEFAULT_LANGUAGE } from "./languages";
import { detectSystemLanguage, normalizeLanguage } from "./system-language";

/**
 * The logic under test mirrors `initialLanguage()` in `./index`.
 *
 * It is duplicated here rather than imported because importing `./index`
 * runs `i18next.init()` as a module side effect, which would leak state into
 * the other i18n tests. The duplication is deliberate and narrow: it asserts
 * the *contract* (persisted value wins; locale is the fallback) that
 * `initialLanguage` implements.
 */
function resolveInitialLanguage(
  storage: { getItem: (k: string) => string | null },
  preferences: readonly string[],
): string {
  try {
    const raw = storage.getItem("weave.ui");
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
  return detectSystemLanguage(preferences);
}

describe("initial language resolution", () => {
  let store: Record<string, string | null>;

  beforeEach(() => {
    store = {};
  });

  afterEach(() => {
    store = {};
  });

  const storage = () => ({ getItem: (k: string) => store[k] ?? null });

  it("uses the persisted language so the first paint is already correct", () => {
    // zustand's persist middleware stores `{ state: {...}, version: 0 }`.
    store["weave.ui"] = JSON.stringify({
      state: { language: "zh" },
      version: 0,
    });
    expect(resolveInitialLanguage(storage(), ["en-US"])).toBe("zh");
  });

  it("falls back to the system locale on a first run", () => {
    // No persisted value at all — a fresh install.
    expect(resolveInitialLanguage(storage(), ["zh-CN", "en-US"])).toBe("zh");
  });

  it("ignores an unsupported persisted value and re-detects", () => {
    // A locale that is no longer supported must not stick around.
    store["weave.ui"] = JSON.stringify({ state: { language: "ja" } });
    expect(resolveInitialLanguage(storage(), ["zh-CN"])).toBe("zh");
  });

  it("survives corrupt persisted JSON", () => {
    store["weave.ui"] = "{not json";
    expect(resolveInitialLanguage(storage(), ["zh-CN"])).toBe("zh");
  });

  it("falls back to DEFAULT_LANGUAGE when nothing else resolves", () => {
    expect(resolveInitialLanguage(storage(), ["de-DE", "fr"])).toBe(
      DEFAULT_LANGUAGE,
    );
  });
});
