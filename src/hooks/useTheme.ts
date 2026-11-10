import { useEffect } from "react";

import { invoke, inTauri } from "@/lib/tauri";
import { useAppStore, ThemeCode } from "@/stores/app";
import {
  THEME_CODES,
  VALID_ACCENT_NAMES,
  applyThemeClass,
} from "@/lib/theme";

// Re-exported for existing call sites (App.tsx / TopBar.tsx / main.tsx).
// The definitions themselves now live in "@/lib/theme" so the app store can
// import the pure DOM helpers without pulling in this hook module.
export { THEME_CODES, applyThemeClass };

function applyAccent(accent: string) {
  const root = document.documentElement;
  // If the accent is valid, or is "theme", use it. Otherwise fall back to "indigo".
  const hasAccent = VALID_ACCENT_NAMES.includes(accent);
  const resolvedAccent = hasAccent || accent === "theme" ? accent : "indigo";
  root.dataset.accent = resolvedAccent;
}

// Subscribe directly to the store so any theme change is reflected on the
// <html> element synchronously, even if no React component re-renders.
let subscribed = false;
function ensureSubscribed() {
  if (subscribed) return;
  subscribed = true;
  let lastTheme = useAppStore.getState().theme;
  let lastAccent = useAppStore.getState().accent;
  useAppStore.subscribe((state) => {
    if (state.theme !== lastTheme) {
      lastTheme = state.theme;
      applyThemeClass(state.theme);
    }
    if (state.accent !== lastAccent) {
      lastAccent = state.accent;
      applyAccent(state.accent);
    }
  });
}

export function useApplyTheme() {
  const theme = useAppStore((s) => s.theme);
  const accent = useAppStore((s) => s.accent);
  useEffect(() => {
    ensureSubscribed();
    applyThemeClass(theme);
    applyAccent(accent);
  }, [theme, accent]);
}

export async function persistTheme(theme: ThemeCode) {
  applyThemeClass(theme);
  if (!inTauri) return;
  await invoke<void>("config_set", {
    path: "settings.general.appearance.theme",
    value: theme,
  }).catch(() => undefined);
}
