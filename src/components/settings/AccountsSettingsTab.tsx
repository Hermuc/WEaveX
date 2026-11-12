import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "@/i18n/hooks";
import { Trash2, AlertCircle, UserPlus, RefreshCw } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { EVENTS } from "@/lib/events";

import { inTauri, invoke, tryInvoke, tryInvokeAction } from "@/lib/tauri";
import { pushToast } from "@/stores/toasts";
import { useAppStore } from "@/stores/app";
import { useConfirm } from "@/hooks/useConfirm";
import { useSteamSessionStore, SteamAccountInfo } from "@/stores/steam-session";
import { triggerGlobalRefresh } from "@/stores/refresh";
import { Tooltip } from "@/components/common/Tooltip";

import DownloadLoginModal from "@/components/dialogs/DownloadLoginModal";

const persist = async (path: string, value: unknown) => {
  if (!inTauri) return;
  await invoke("config_set", { path, value }).catch(() => undefined);
};

/**
 * The account page.
 *
 * Web-session login and the download account used to be two separate sections,
 * but they now resolve to the same Steam account: the parser session and the
 * downloader both use the account the user added here. Showing it twice was
 * duplicate state that could disagree with itself, so this renders one block
 * that owns the account, its live session status and every action on it.
 */
export default function AccountsSettingsTab({
  onOpenParser,
}: {
  onOpenParser: () => void;
}) {
  const { t } = useTranslation();

  return (
    <div className="space-y-5 p-4">
      <div className="space-y-2">
        <p className="text-xs font-medium tracking-wide text-subtle uppercase">
          {t("settings.account_section") || "Steam account"}
        </p>
        <p className="text-xs text-muted">
          {t("settings.account_section_description") ||
            "One account signs in for browsing and is used for downloads."}
        </p>
        <AccountSection onOpenParser={onOpenParser} />
      </div>
    </div>
  );
}

function AccountSection({ onOpenParser }: { onOpenParser: () => void }) {
  const { t } = useTranslation();
  const { confirm: showConfirm, ConfirmDialog } = useConfirm();
  const state = useAppStore();
  const setAccounts = useAppStore((s) => s.setAccounts);
  const sessionPhase = useSteamSessionStore((s) => s.phase);
  const account = useSteamSessionStore((s) => s.account);
  const setSessionLoggedIn = useSteamSessionStore((s) => s.setLoggedIn);

  const [busy, setBusy] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);

  const refreshSession = useCallback(async () => {
    if (!inTauri) return;
    const v = await tryInvoke<boolean>("steam_is_logged_in", undefined, false);
    if (!v) {
      setSessionLoggedIn(null);
      return;
    }
    const info = await tryInvoke<SteamAccountInfo | null>(
      "steam_current_account",
      undefined,
      null,
    );
    setSessionLoggedIn(info ?? null);
  }, [setSessionLoggedIn]);

  const refreshAccounts = useCallback(async () => {
    if (!inTauri) return;
    const all = await tryInvoke<
      { index: number; username: string; is_custom: boolean }[]
    >("accounts_list", undefined, []);
    if (all) setAccounts(all);
  }, [setAccounts]);

  useEffect(() => {
    void refreshSession();
    void refreshAccounts();
  }, [refreshSession, refreshAccounts]);

  // A sign-in from the parser window updates both the session and the list.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let active = true;

    if (inTauri) {
      void listen(EVENTS.STEAM_LOGIN_SUCCESS, async () => {
        await invoke<number>("steam_sync_cookies").catch(() => 0);
        pushToast(
          t("messages.signed_in_to_steam") || "Signed in to Steam",
          "success",
        );
        await refreshAccounts();
        await refreshSession();
        triggerGlobalRefresh();
      }).then((fn) => {
        if (active) {
          unlisten = fn;
        } else {
          fn();
        }
      });
    }

    return () => {
      active = false;
      if (unlisten) unlisten();
    };
  }, [t, refreshAccounts, refreshSession]);

  const openParser = async () => {
    if (!inTauri) return;
    await invoke<void>("steam_parser_show").catch(() => undefined);
  };

  // There is no bundled pool any more: re-login needs a stored account.
  const relogin = async () => {
    if (!inTauri) return;
    const stored = await tryInvoke<
      { index: number; username: string; is_custom: boolean }[]
    >("accounts_list", undefined, []);
    if (!stored || stored.length === 0) {
      pushToast(
        t("settings.add_account_first") ||
          "Add your Steam account below first, then you can re-login.",
        "error",
      );
      return;
    }
    setBusy(true);
    try {
      const res = await tryInvokeAction<boolean>("steam_auto_login", {
        accountIndex: null,
        force: true,
      });
      if (res.ok && res.value) {
        // Let WebView2 flush new cookies before re-reading them.
        await invoke<number>("steam_sync_cookies").catch(() => 0);
        let info = await tryInvoke<SteamAccountInfo | null>(
          "steam_current_account",
          undefined,
          null,
        );
        if (!info) {
          await new Promise((r) => setTimeout(r, 1500));
          await invoke<number>("steam_sync_cookies").catch(() => 0);
          info = await tryInvoke<SteamAccountInfo | null>(
            "steam_current_account",
            undefined,
            null,
          );
        }
        if (!info) {
          pushToast(
            t("messages.error") || "Login failed — session invalid",
            "error",
          );
          return;
        }
        pushToast(
          t("messages.signed_in_to_steam") || "Signed in to Steam",
          "success",
        );
        await refreshAccounts();
        await refreshSession();
        triggerGlobalRefresh();
      } else {
        const err = !res.ok
          ? res.error
          : "Steam rejected the credentials for the account in use. Add your own account below, then retry.";
        pushToast(`${t("messages.error") || "Relogin failed"}: ${err}`, "error");
      }
    } catch (e) {
      const err = e instanceof Error ? e.message : String(e);
      pushToast(`${t("messages.error") || "Relogin failed"}: ${err}`, "error");
    } finally {
      setBusy(false);
    }
  };

  const remove = async (username: string, index: number) => {
    if (!inTauri) return;
    const confirmed = await showConfirm({
      title: t("labels.remove_account") || "Remove Account",
      message:
        t("settings.confirm_remove_account") ||
        t("labels.remove_account_question", { user: username }),
      confirmLabel: t("buttons.remove") || "Remove",
      cancelLabel: t("buttons.cancel") || "Cancel",
      variant: "danger",
    });
    if (!confirmed) return;
    const res = await tryInvokeAction("accounts_remove_custom", { username });
    if (res.ok) {
      pushToast(t("messages.removed"), "success");
      if (state.accountIndex === index) {
        state.setAccountIndex(0);
        void persist("settings.account.account.account_number", 0);
      }
      await refreshAccounts();
    } else {
      pushToast(
        `${t("messages.error") || "Could not remove account"}: ${res.error}`,
        "error",
      );
    }
  };

  const loggedIn = sessionPhase === "logged-in" && account;
  const displayName =
    account?.persona_name?.trim() || account?.account_name?.trim() || "";
  const customAccounts = state.accounts.filter((a) => a.is_custom);
  const hasAccount = customAccounts.length > 0;

  return (
    <div className="flex flex-col gap-3 rounded-md border border-border bg-surface-sunken p-3">
      {/* Identity + live session status, merged from the old session row. */}
      <div className="flex items-center justify-between gap-3">
        {loggedIn ? (
          <div className="flex items-center gap-1 text-sm">
            {account.avatar_url && (
              <img
                src={account.avatar_url}
                alt=""
                className="mr-1 size-8 rounded-full object-cover"
              />
            )}
            <span className="leading-none font-medium">{displayName}</span>
            <Tooltip content={t("settings.relogin") || "Relogin"} side="top">
              <button
                className="btn-ghost h-auto p-1.5 text-muted hover:text-foreground"
                onClick={relogin}
                disabled={busy || !inTauri}
              >
                <RefreshCw className={`size-4 ${busy ? "animate-spin" : ""}`} />
              </button>
            </Tooltip>
          </div>
        ) : (
          <div className="flex items-center gap-2 text-[11px]">
            <div className="flex items-center gap-2 rounded-md border border-danger/40 bg-danger/10 px-2.5 py-1.5">
              <AlertCircle className="size-3.5 shrink-0 text-danger" />
              <span className="truncate text-danger">
                {hasAccount
                  ? t("settings.not_signed_in") || "Not signed in"
                  : t("settings.no_account") || "No account added"}
              </span>
            </div>
            {hasAccount && (
              <Tooltip content={t("settings.relogin") || "Relogin"} side="top">
                <button
                  className="btn-ghost h-auto p-2 text-muted hover:text-foreground"
                  onClick={relogin}
                  disabled={busy || !inTauri}
                >
                  <RefreshCw className={`size-5 ${busy ? "animate-spin" : ""}`} />
                </button>
              </Tooltip>
            )}
          </div>
        )}
        {import.meta.env.DEV && (
          <div className="flex gap-2">
            <button className="btn-outline" onClick={openParser} disabled={!inTauri}>
              {t("settings.open_parser") || "Open Parser"}
            </button>
            <button className="btn-outline" onClick={onOpenParser} disabled={!inTauri}>
              {t("settings.parser_logs") || "Parser Logs"}
            </button>
          </div>
        )}
      </div>

      {/* The stored account(s). One entry in practice; the list keeps delete
          and selection meaningful if more are ever allowed again. */}
      {hasAccount && (
        <div className="divide-y divide-border rounded-md border border-border">
          {customAccounts.map((a) => (
            <div
              key={`${a.index}-${a.username}`}
              className="flex items-center gap-3 p-2.5 text-sm"
            >
              <span className="flex flex-1 items-center gap-2">
                <span>{a.username}</span>
                <span className="chip text-info">
                  {t("settings.custom_badge") || "custom"}
                </span>
              </span>
              <button
                className="btn-ghost text-error"
                onClick={() => remove(a.username, a.index)}
                title={t("settings.remove_account") || "Remove account"}
              >
                <Trash2 className="size-4" />
              </button>
            </div>
          ))}
        </div>
      )}

      <div className="flex items-center gap-2">
        <button
          className="btn-outline inline-flex items-center gap-1.5"
          onClick={() => setModalOpen(true)}
          disabled={!inTauri}
        >
          <UserPlus className="size-4" />
          {t("settings.add_custom_account") || "Add custom account"}
        </button>
      </div>

      <DownloadLoginModal
        open={modalOpen}
        onOpenChange={setModalOpen}
        onSuccess={async () => {
          await refreshAccounts();
          await refreshSession();
        }}
      />
      {ConfirmDialog}
    </div>
  );
}