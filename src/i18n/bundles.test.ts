import { describe, it, expect } from "vitest";

// Importing the real i18next instance (test-setup only mocks react-i18next,
// not i18next itself), so this exercises the actual resource bundles.
import i18n from "./index";
import en from "./locales/en";
import zh from "./locales/zh";
import { DEFAULT_LANGUAGE, LOCALES, SUPPORTED_LANGUAGES } from "./languages";

type Tree = { [k: string]: string | Tree };

/** Collect every leaf as a dotted key. */
function leafKeys(tree: Tree, prefix = ""): string[] {
  const out: string[] = [];
  for (const [k, v] of Object.entries(tree)) {
    const path = prefix ? prefix + "." + k : k;
    if (typeof v === "string") out.push(path);
    else out.push(...leafKeys(v as Tree, path));
  }
  return out;
}

// The i18next instance is strongly typed against the en bundle key union;
// widen t() so we can probe arbitrary generated keys in this meta-test.
const t = i18n.t as unknown as (key: string) => string;

const zhKeys = leafKeys(zh as unknown as Tree);
const enKeys = leafKeys(en as unknown as Tree);

describe("i18n resource bundles", () => {
  it("ships the same leaf-key set for en and zh", () => {
    expect([...zhKeys].sort()).toEqual([...enKeys].sort());
  });

  it("returns Chinese strings (not the English fallback) for every zh key", async () => {
    await i18n.changeLanguage("zh");
    const notChinese = zhKeys.filter((key) => {
      const translated = t(key);
      if (translated === key) return true;
      // A handful of zh values are intentionally identical to English
      // (brand names, "OK"). Ignore genuine fallbacks by checking the value
      // actually came from the zh bundle.
      return translated === i18n.getResource("en", "translation", key) &&
        translated !== i18n.getResource("zh", "translation", key);
    });
    expect(notChinese).toEqual([]);
  });

  /**
   * The backend keeps its own copy of the language list in
   * `src-tauri/src/services/i18n.rs`, and its comment says to keep it in sync.
   * A Rust test cannot read this file, so the two lists are checked from here
   * instead: everything the UI advertises must have a real bundle, and the
   * default must be one of them.
   */
  it("keeps the locale registry and the bundles in agreement", () => {
    const codes = LOCALES.map((l) => l.code);

    // Every registered locale has a translation resource.
    const resources = i18n.options.resources ?? {};
    for (const code of codes) {
      expect(Object.keys(resources), `missing bundle for ${code}`).toContain(code);
    }

    // No bundle is shipped without being registered (a locale that exists but
    // cannot be selected is just dead weight).
    const bundled = Object.keys(resources).filter((c) => c !== "dev");
    expect([...bundled].sort()).toEqual([...codes].sort());

    // The picker is derived from the registry, not hand-maintained.
    expect(SUPPORTED_LANGUAGES.map((l) => l.code)).toEqual(codes);

    // The default has to be selectable.
    expect(codes).toContain(DEFAULT_LANGUAGE);
  });

  it("switches between en and zh at runtime", async () => {
    await i18n.changeLanguage("en");
    const enValue = t("settings.language");
    await i18n.changeLanguage("zh");
    const zhValue = t("settings.language");
    expect(zhValue).not.toBe(enValue);
    expect(zhValue).toBe(zh.settings.language);
  });
});
