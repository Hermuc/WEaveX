import { describe, it, expect } from "vitest";

// Importing the real i18next instance (test-setup only mocks react-i18next,
// not i18next itself), so this exercises the actual resource bundles.
import i18n from "./index";
import en from "./locales/en";
import ru from "./locales/ru";
import zh from "./locales/zh";

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
const ruKeys = leafKeys(ru as unknown as Tree);

describe("i18n resource bundles", () => {
  it("ships the same leaf-key set for en, ru and zh", () => {
    expect([...zhKeys].sort()).toEqual([...enKeys].sort());
    expect([...ruKeys].sort()).toEqual([...enKeys].sort());
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

  it("switches between ru and zh at runtime", async () => {
    await i18n.changeLanguage("ru");
    const ruValue = t("settings.language");
    await i18n.changeLanguage("zh");
    const zhValue = t("settings.language");
    expect(zhValue).not.toBe(ruValue);
    expect(zhValue).toBe(zh.settings.language);
  });
});
