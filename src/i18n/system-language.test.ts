import { describe, it, expect } from "vitest";

import {
  browserLanguagePreferences,
  detectSystemLanguage,
  normalizeLanguage,
} from "./system-language";

describe("normalizeLanguage", () => {
  it("matches exact supported codes", () => {
    expect(normalizeLanguage("en")).toBe("en");
    expect(normalizeLanguage("zh")).toBe("zh");
  });

  it("is case-insensitive and treats underscores as dashes", () => {
    expect(normalizeLanguage("ZH")).toBe("zh");
    expect(normalizeLanguage("zh_CN")).toBe("zh");
    expect(normalizeLanguage("EN-us")).toBe("en");
  });

  it("falls back to the primary subtag", () => {
    expect(normalizeLanguage("zh-CN")).toBe("zh");
    expect(normalizeLanguage("zh-Hans-CN")).toBe("zh");
    expect(normalizeLanguage("zh-TW")).toBe("zh");
    expect(normalizeLanguage("en-US")).toBe("en");
  });

  it("returns null for unsupported or empty input", () => {
    expect(normalizeLanguage("de-DE")).toBeNull();
    expect(normalizeLanguage("fr")).toBeNull();
    // Russian used to ship; it was dropped, so it must now be rejected.
    expect(normalizeLanguage("ru")).toBeNull();
    expect(normalizeLanguage("")).toBeNull();
    expect(normalizeLanguage(null)).toBeNull();
    expect(normalizeLanguage(undefined)).toBeNull();
  });
});

describe("detectSystemLanguage", () => {
  it("picks the first supported preference", () => {
    expect(detectSystemLanguage(["zh-CN", "en-US"])).toBe("zh");
    expect(detectSystemLanguage(["en-GB", "zh-CN"])).toBe("en");
  });

  it("skips unsupported languages and keeps searching", () => {
    expect(detectSystemLanguage(["fr-FR", "ja-JP", "zh-TW", "en"])).toBe("zh");
  });

  it("falls back to DEFAULT_LANGUAGE (en) when nothing matches", () => {
    expect(detectSystemLanguage(["de", "fr"])).toBe("en");
    expect(detectSystemLanguage([])).toBe("en");
  });

  it("uses the ambient browser locale when no list is provided", () => {
    // jsdom reports an en-US-like navigator.language by default.
    expect(detectSystemLanguage()).toBe("en");
  });

  it("prefers an explicitly provided OS locale over the browser list", () => {
    // Simulates the bootstrap flow: OS locale first, then browser fallbacks.
    expect(detectSystemLanguage(["zh-CN", "en-US"])).toBe("zh");
    // An unsupported OS locale must not block the browser fallback.
    expect(detectSystemLanguage(["de-DE", "zh-CN"])).toBe("zh");
  });
});

describe("browserLanguagePreferences", () => {
  it("returns a non-empty list in a jsdom environment", () => {
    const prefs = browserLanguagePreferences();
    expect(Array.isArray(prefs)).toBe(true);
    expect(prefs.length).toBeGreaterThan(0);
  });
});
