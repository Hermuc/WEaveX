import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { invoke, inTauri, tryInvoke, tryInvokeOk } from "@/lib/tauri";
import { useDotnetStore } from "@/stores/dotnet";
import { pushToast } from "@/stores/toasts";
import { useUpdaterStore } from "@/stores/updater";
import { useAppStore, ThemeCode } from "@/stores/app";
import { TaskPhase, useTasksStore } from "@/stores/tasks";
import { useInstalledStore } from "@/stores/installed";
import { useSteamSessionStore, SteamAccountInfo } from "@/stores/steam-session";
import i18n from "@/i18n";
import {
  browserLanguagePreferences,
  detectSystemLanguage,
  normalizeLanguage,
} from "@/i18n/system-language";
import { maybeMinimize } from "@/lib/window";
import type { UpdateInfo } from "@/types/updater";
import { EVENTS } from "@/lib/events";

export function useBootstrap() {
  const dotnetInitialized = useRef(false);
  void dotnetInitialized;

  useEffect(() => {
    // Show the window before any command runs: the chain below is synchronous
    // on the Rust side, so it runs on the main thread and blocks repainting.
    void tryInvoke("app_restore_window_geometry", undefined, false).then(() =>
      tryInvoke("app_show_main_window", undefined, undefined),
    );

    void (async () => {
      if (!inTauri) {
        useAppStore.setState({ ready: true });
        return;
      }

      const config = await tryInvoke<Record<string, unknown>>("config_get_all");
      const availableLanguages = await tryInvoke<
        { code: string; label: string }[]
      >("i18n_get_available_languages", undefined, []);
      const weDirectory = await tryInvoke<string | null>(
        "we_get_directory",
        undefined,
        null,
      );
      const accountIndex = await tryInvoke<number>(
        "config_get",
        { path: "settings.account.account.account_number" },
        0,
      );
      const infiniteRetryAccounts = await tryInvoke<boolean>(
        "config_get",
        { path: "settings.account.account.infinite_retry_accounts" },
        false,
      );
      const accounts = await tryInvoke<
        { index: number; username: string; is_custom: boolean }[]
      >("accounts_list", undefined, []);

      // Set language from config — auto-detect on first launch
      const savedLanguage = getConfigValue<string | null>(
        config,
        ["general", "appearance", "language"],
        null,
      );
      // Validate the persisted value against the supported set; a stale or
      // unsupported code (e.g. a removed locale) falls back to auto-detection
      // instead of leaving the picker in an invalid state.
      let language = normalizeLanguage(savedLanguage);
      if (!language) {
        // Prefer the OS locale (tauri-plugin-os) over the WebView's
        // navigator.language; the WebView may report the install-time default
        // rather than the user's actual Windows display language. Both are
        // consulted so a missing plugin or odd OS value still resolves.
        const osLocale = await tryInvoke<string>("plugin:os|locale");
        const preferences = [
          ...(osLocale ? [osLocale] : []),
          ...browserLanguagePreferences(),
        ];
        language = detectSystemLanguage(preferences);
      }
      void i18n.changeLanguage(language);

      // Persist the resolved language on first launch or when the stored value
      // was missing/unsupported, so subsequent launches skip detection.
      if (language !== savedLanguage && inTauri) {
        void invoke("config_set", {
          path: "settings.general.appearance.language",
          value: language,
        }).catch(() => undefined);
      }

      const appearance = getConfigValue<Record<string, unknown>>(
        config,
        ["general", "appearance"],
        {},
      );
      const patch: Record<string, unknown> = {
        weDirectory: weDirectory ?? "",
        availableLanguages: availableLanguages ?? [],
        accountIndex: typeof accountIndex === "number" ? accountIndex : 0,
        infiniteRetryAccounts: typeof infiniteRetryAccounts === "boolean" ? infiniteRetryAccounts : false,
        accounts: accounts ?? [],
        language: language,
        ready: true,
      };
      // Only override persisted theme/accent when the backing config has an
      // explicit value. Otherwise we would clobber the user's in-session
      // selection on next bootstrap.
      if (appearance.theme) patch.theme = appearance.theme as ThemeCode;
      if (appearance.accent) patch.accent = appearance.accent as string;
      const behavior = getConfigValue<Record<string, unknown>>(
        config,
        ["general", "behavior"],
        {},
      );
      if (typeof behavior.show_active_filters === "boolean") {
        patch.showActiveFilters = behavior.show_active_filters;
      }
      useAppStore.setState(patch);

      // Sign the hidden Steam webview in on startup so 18+ content is visible
      // and the scraper has a valid session. `accountIndex: null` tells the
      // backend to use the first account the user added (there is no bundled
      // fallback), so no separate parser account exists. If login requires
      // Steam Guard or the password is stale this returns false — the user can
      // still sign in from Settings → Account.
      //
      // We drive the sidebar status section from this lifecycle: spinner
      // while in flight, then signed-in / unknown-account / error once it
      // settles. Kept fire-and-forget so it never blocks the rest of boot.
      void syncSteamSession();

      // Disabled for the bisect: the dotnet probe spawns a child process
      // (which stalls the parent on Windows) and the installed scan is heavy.
      // void maybeCheckForUpdates();
      // if (!dotnetInitialized.current) {
      //   dotnetInitialized.current = true;
      //   void invoke("dotnet_init").catch(() => undefined);
      //   void invoke("plugins_init").catch(() => undefined);
      // }
      // void registerWindowStatePersistence();
      // void useInstalledStore.getState().refresh();

      await Promise.all([
        listen<{
          phase: string;
          message: string;
          progress?: number | null;
        }>(EVENTS.DOTNET_STATUS, (event) => {
          useDotnetStore.getState().setStatus({
            phase: event.payload.phase as any,
            message: event.payload.message,
            progress: event.payload.progress ?? null,
          });
        }),
        listen<{
          phase: string;
          plugin_id: string;
          plugin_name: string;
          message: string;
          progress?: number | null;
        }>(EVENTS.PLUGIN_STATUS, (event) => {
          useDotnetStore.getState().setPluginStatus({
            phase: event.payload.phase as any,
            plugin_id: event.payload.plugin_id,
            plugin_name: event.payload.plugin_name,
            message: event.payload.message,
            progress: event.payload.progress ?? null,
          });
        }),
        listen<{
          pubfileid: string;
          status: string;
          account: string;
          phase: string;
          progress?: number | null;
        }>(EVENTS.DOWNLOAD_STATUS, (event) => {
          const normalizedPhase = normalizeTaskPhase(event.payload.phase);
          useTasksStore.getState().upsert({
            ...event.payload,
            kind: "download",
            phase: normalizedPhase,
          });
          if (normalizedPhase === "failed") {
            pushToast(
              i18n.t("bootstrap.download_failed", {
                id: event.payload.pubfileid,
                status: event.payload.status,
              }),
              "error",
            );
          } else if (normalizedPhase === "cancelled") {
            pushToast(
              i18n.t("bootstrap.download_cancelled", {
                id: event.payload.pubfileid,
              }),
              "warning",
            );
          } else if (normalizedPhase === "completed") {
            pushToast(
              i18n.t("bootstrap.download_completed", {
                id: event.payload.pubfileid,
              }),
              "success",
            );
            // The newly-downloaded item should now show the Installed
            // indicator on cards regardless of which view we're in.
            // We refresh the installed store so cards update their
            // installed indicators, and fetch workshop metadata for
            // the new item so it's cached for future use.
            void useInstalledStore.getState().refresh();
            void maybeAutoApply(event.payload.pubfileid);
          }
        }),

        listen<{ payload: string }>(
          EVENTS.DOWNLOAD_REQUIRE_APP_CONFIRM,
          () => {
            pushToast(i18n.t("bootstrap.steam_app_confirm"), "info");
          },
        ),
        listen<{
          pubfileid: string;
          status: string;
          account: string;
          phase: string;
          progress?: number | null;
        }>(EVENTS.EXTRACT_STATUS, (event) => {
          const normalizedPhase = normalizeTaskPhase(event.payload.phase);
          useTasksStore.getState().upsert({
            ...event.payload,
            kind: "extract",
            phase: normalizedPhase,
          });
          if (normalizedPhase === "failed") {
            pushToast(
              i18n.t("bootstrap.extract_failed", {
                id: event.payload.pubfileid,
                status: event.payload.status,
              }),
              "error",
            );
          } else if (normalizedPhase === "completed") {
            pushToast(
              i18n.t("bootstrap.extract_completed", {
                id: event.payload.pubfileid,
              }),
              "success",
            );
          }
        }),
      ]);
    })();
  }, []);
}

function getConfigValue<T>(
  config: Record<string, unknown> | null | undefined,
  path: string[],
  fallback: T,
): T {
  let current: unknown = config;
  for (const key of path) {
    if (
      typeof current !== "object" ||
      current === null ||
      !Object.prototype.hasOwnProperty.call(current, key)
    ) {
      return fallback;
    }
    current = (current as Record<string, unknown>)[key];
  }
  return current === undefined || current === null ? fallback : (current as T);
}

function normalizeTaskPhase(value: string): TaskPhase {
  if (
    value === "starting" ||
    value === "running" ||
    value === "completed" ||
    value === "failed" ||
    value === "cancelled"
  ) {
    return value;
  }
  return "running";
}

export async function changeLanguageTo(code: string) {
  useAppStore.setState({ language: code });
  await i18n.changeLanguage(code);
  if (inTauri) {
    await invoke<void>("config_set", {
      path: "settings.general.appearance.language",
      value: code,
    }).catch(() => undefined);
    await invoke<void>("i18n_set_language", { language: code }).catch(
      () => undefined,
    );
  }
}

/**
 * Run the parser auto-login and reflect every stage into the Steam-session
 * store so the sidebar status section can render a spinner / signed-in /
 * unknown-account / error state. Pure UI plumbing — the actual session and
 * cookie handling stays in the backend.
 */
async function syncSteamSession() {
  const session = useSteamSessionStore.getState();
  if (!inTauri) {
    session.setPhase("idle");
    return;
  }
  session.setLoggingIn();


  // Hard ceiling on the whole startup sign-in dance.
  //
  // The steps below can each block for a long time (the legacy form-fill path
  // alone spends 40s polling plus 12s on a community probe). If any of them
  // wedges, the sidebar stayed on "signing in…" forever with no way for the
  // user to tell whether the app was working. Cap it and settle on a neutral
  // state instead of spinning indefinitely.
  const SETTLE_AFTER_MS = 45_000;
  const settleTimer = setTimeout(() => {
    const s = useSteamSessionStore.getState();
    if (s.phase === "logging-in") s.setSignedOut();
  }, SETTLE_AFTER_MS);

  // Probe with retries before concluding the user is signed out.
  //
  // A single failed request is usually a network blip (or Steam rate-limiting
  // the profile endpoint), not an invalid session. Treating it as "logged out"
  // opened the login dialog, which used to wipe the stored cookies and turn a
  // transient hiccup into a permanent sign-out.
  type Probe =
    | { state: "signed_in"; account: SteamAccountInfo }
    | { state: "signed_out" }
    | { state: "unknown" };

  /// Probe the session, telling "failed to ask" apart from "not signed in".
  ///
  /// The old version used `steam_current_account`, whose `None` covers both a
  /// genuinely anonymous session and a failed HTTP call. Because Steam's profile
  /// page regularly takes 3-4s on this network, a slow start looked exactly like
  /// a sign-out and raised a false "login failed" badge over a working account.
  const probeAccount = async (): Promise<Probe> => {
    let sawUnknown = false;
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const res = await tryInvoke<Probe>(
        "steam_probe_account",
        undefined,
        { state: "unknown" },
      );
      if (res?.state === "signed_in") return res;
      if (res?.state === "signed_out") return res;
      sawUnknown = true;
      if (attempt < 2) {
        await new Promise((r) => setTimeout(r, 800 * (attempt + 1)));
      }
    }
    return sawUnknown ? { state: "unknown" } : { state: "signed_out" };
  };

  try {
    // 1. Ask Steam who we are.
    let probe = await probeAccount();

    if (probe.state === "signed_in") {
      session.setLoggedIn(probe.account);
      return;
    }

    // If the probe could not reach Steam we know nothing. Leaving the store in
    // its previous state is the honest answer: a red "login failed" badge for
    // a network hiccup is exactly the bug this replaces.
    if (probe.state === "unknown") {
      session.setUnknownOrKeep();
      return;
    }

    // 2. Nobody signed in per Steam. Try an OAuth session saved at the last
    //    successful login — `{steamid, access_token}` rebuilds `steamLoginSecure`
    //    and skips both the login page and Steam Guard.
    const restored = await tryInvoke<boolean>(
      "steam_restore_session",
      { username: null },
      false,
    );
    if (restored) {
      probe = await probeAccount();
      if (probe.state === "signed_in") {
        session.setLoggedIn(probe.account);
        return;
      }
    }

    // 3. Fall back to the stored credentials (may hit Steam Guard).
    let ok = await tryInvoke<boolean>(
      "steam_auto_login",
      { accountIndex: null },
      false,
    );

    // NEVER escalate to `force: true` here.
    //
    // `force` makes the backend run `logout_locked()`, which calls
    // `clear_all_browsing_data()` and destroys `steamLoginSecure`. Doing that
    // from a *startup heuristic* is how a signed-in user got logged out: the
    // probe above already failed, so the retry could not confirm the session,
    // and the force-re-login wiped a cookie that was perfectly valid. A forced
    // re-login is only ever acceptable when the user explicitly asks for it.
    if (ok) {
      probe = await probeAccount();
      if (probe.state === "signed_in") {
        session.setLoggedIn(probe.account);
        return;
      }
    }

    {
      // Distinguish "no account" from "signing in went wrong".
      //
      // Nothing here actually failed: we simply do not have credentials, or
      // the automatic attempt could not complete without the user (Steam
      // Guard). The Workshop stays browsable anonymously, so flagging this as
      // an error painted a red "login failed" badge over a perfectly working
      // app and made users think it was broken.
      //
      // A real failure is reported by the interactive login paths below
      // (`steam_oauth_begin` / `steam_oauth_submit_guard`), which call
      // `setError()` themselves.
      // Do NOT clear stored state: the saved session may still be valid and a
      // later probe can pick it up.
      session.setSignedOut();
    }
  } catch {
    session.setError();
  } finally {
    clearTimeout(settleTimer);
  }
}

export async function maybeCheckForUpdates() {
  if (!inTauri) return;
  const enabled = await tryInvoke<boolean>(
    "config_get",
    { path: "settings.general.behavior.auto_check_updates" },
    true,
  );
  if (!enabled) return;
  const info = await tryInvoke<UpdateInfo>("updater_check", undefined);
  if (info?.update_available) {
    useUpdaterStore.getState().show(info);
  }
}

async function maybeAutoApply(pubfileid: string) {
  if (!inTauri || !pubfileid) return;
  const enabled = await tryInvoke<boolean>(
    "config_get",
    { path: "settings.general.behavior.auto_apply_last_downloaded" },
    false,
  );
  if (!enabled) return;
  // Need the freshly-installed wallpaper's project.json path. Pull the
  // current installed list and find the matching pubfileid.
  type Installed = { pubfileid: string; project_json_path: string };
  const installed = await tryInvoke<Installed[]>(
    "we_list_installed",
    undefined,
    [],
  );
  const match = (installed ?? []).find((w) => w.pubfileid === pubfileid);
  if (!match) return;
  const ok = await tryInvokeOk("we_apply", {
    projectPath: match.project_json_path,
    monitor: null,
    force: false,
  });
  if (ok) {
    void maybeMinimize();
  }
}

export async function registerWindowStatePersistence() {
  if (!inTauri) return;
  try {
    const win = getCurrentWindow();

    const save = async () => {
      const enabled = await tryInvoke<boolean>(
        "config_get",
        { path: "settings.general.behavior.save_window_state" },
        true,
      );
      if (!enabled) return;

      // Don't save if window is minimized (positions are -32000)
      const pos = await win.outerPosition();
      if (pos.x < -30000 || pos.y < -30000) {
        return;
      }

      const [size, maximized] = await Promise.all([
        win.outerSize(),
        win.isMaximized(),
      ]);
      void invoke("app_save_window_geometry", {
        geom: {
          x: pos.x,
          y: pos.y,
          width: size.width,
          height: size.height,
          is_maximized: Boolean(maximized),
        },
      });
    };

    // Save periodically when window state changes
    let saveTimeout: ReturnType<typeof setTimeout> | null = null;
    const debouncedSave = () => {
      if (saveTimeout) clearTimeout(saveTimeout);
      saveTimeout = setTimeout(() => {
        void save();
      }, 500);
    };

    await win.onResized(() => debouncedSave());
    await win.onMoved(() => debouncedSave());
  } catch (err) {
    console.warn("window state persistence setup failed", err);
  }
}
