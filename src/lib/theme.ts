import type { ThemeCode } from "@/stores/app";

/**
 * Pure theme primitives — no React, no store imports at runtime.
 *
 * These used to live in `src/hooks/useTheme.ts`, which forced
 * `src/stores/app.ts` to import a React hook module just to call
 * `applyThemeClass`. That created a cyclic dependency
 * (stores/app.ts -> hooks/useTheme.ts -> stores/app.ts).
 * Keeping the DOM side-effects here lets both the store and the hook depend
 * on this leaf module instead. The `ThemeCode` import above is type-only,
 * so it is erased at compile time and does not reintroduce the cycle.
 */

export const THEME_CODES: ThemeCode[] = [
  "dark",
  "light",
  "nord",
  "solarized",
  "black",
];

/** Accent names accepted by `applyAccent`; anything else falls back to "indigo". */
export const VALID_ACCENT_NAMES = [
  "indigo",
  "blue",
  "purple",
  "pink",
  "rose",
  "orange",
  "amber",
  "emerald",
  "teal",
  "cyan",
];

/**
 * Apply a theme to the document root/body and remember it in localStorage.
 * Pure DOM side-effect: reads/writes no React state.
 */
export function applyThemeClass(theme: ThemeCode) {
  const root = document.documentElement;
  const body = document.body;
  THEME_CODES.forEach((t) => {
    root.classList.remove(`theme-${t}`);
    body?.classList.remove(`theme-${t}`);
  });
  root.classList.add(`theme-${theme}`);
  body?.classList.add(`theme-${theme}`);
  if (theme === "light") {
    root.classList.remove("dark");
    body?.classList.remove("dark");
  } else {
    root.classList.add("dark");
    body?.classList.add("dark");
  }
  root.dataset.theme = theme;

  try {
    localStorage.setItem("weave.theme", theme);
  } catch {
    /* noop */
  }
}
