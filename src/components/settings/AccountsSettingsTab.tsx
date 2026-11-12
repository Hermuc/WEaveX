import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "@/i18n/hooks";
import { LogIn, LogOut, UserPlus } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { EVENTS } from "@/lib/events";

import { inTauri, invoke, tryInvoke, tryInvokeAction } from "@/lib/tauri";
import { pushToast } from "@/stores/toasts";
import { useAppStore } from "@/stores/app";
import { useConfirm } from "@/hooks/useConfirm";
import { useSteamSessionStore, SteamAccountInfo } from "@/stores/steam-session";
import { triggerGlobalRefresh } from "@/stores/refresh";

import DownloadLoginModal from "@/components/dialogs/DownloadLoginModal";

const persist = async (path: string, value: unknown) => {
  if (!inTauri) return;
  await invoke("config_set", { path, value }).catch(() => undefined);
};

/**
 * The account page.
 *
 * One account signs in for browsing and is used for downloads, so this renders
 * exactly one card with exactly one action: sign in when signed out, sign out
 * when signed in. There is no separate "add account" step and no second relogin
 * control -- the saved credential and the live session are the same thing from
 * the user's point of view.
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
        <AccountCard onOpenParser={onOpenParser} />
      </div>
    </div>
  );
}

function AccountCard({ onOpenParser }: { onOpenParser: () => void }) {
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

  const savedAccounts = state.accounts.filter((a) => a.is_custom);
  const savedUsername = savedAccounts[0]?.username ?? "";
  const loggedIn = sessionPhase === "logged-in" && !!account;
  const displayName =
    account?.persona_name?.trim() || account?.account_name?.trim() || savedUsername;

  // One button, one meaning: sign in when signed out, sign out when signed in.
  const primaryAction = async () => {
    if (!inTauri || busy) return;

    if (loggedIn) {
      const confirmed = await showConfirm({
        title: t("settings.sign_out") || "Sign out",
        message:
          t("settings.confirm_sign_out") ||
          "Sign out of Steam? Browsing age-gated items and downloads will stop until you sign in again.",
        confirmLabel: t("settings.sign_out") || "Sign out",
        cancelLabel: t("buttons.cancel") || "Cancel",
        variant: "danger",
      });
      if (!confirmed) return;
      setBusy(true);
      try {
        await invoke("steam_login_prepare").catch(() => undefined);
        await refreshSession();
      } finally {
        setBusy(false);
      }
      return;
    }

    // Signed out: the login dialog is the single entry point.
    setModalOpen(true);
  };

  // Removing the saved credential is a separate, destructive intent.
  const forgetAccount = async () => {
    if (!inTauri || !savedUsername) return;
    const confirmed = await showConfirm({
      title: t("labels.remove_account") || "Remove account",
      message:
        t("settings.confirm_remove_account") ||
        t("labels.remove_account_question", { user: savedUsername }),
      confirmLabel: t("buttons.remove") || "Remove",
      cancelLabel: t("buttons.cancel") || "Cancel",
      variant: "danger",
    });
    if (!confirmed) return;
    const res = await tryInvokeAction("accounts_remove_custom", {
      username: savedUsername,
    });
    if (res.ok) {
      pushToast(t("messages.removed"), "success");
      state.setAccountIndex(0);
      void persist("settings.account.account.account_number", 0);
      await refreshAccounts();
      await refreshSession();
    } else {
      pushToast(
        `${t("messages.error") || "Could not remove account"}: ${res.error}`,
        "error",
      );
    }
  };

  return (
    <div className="flex flex-col gap-3 rounded-md border border-border bg-surface-sunken p-3">
      <div className="flex items-center gap-3">
        {loggedIn && account?.avatar_url ? (
          <img
            src={account.avatar_url}
            alt=""
            className="size-9 rounded-full object-cover"
          />
        ) : (
          <div className="flex size-9 items-center justify-center rounded-full bg-surface-raised text-subtle">
            <UserPlus className="size-4" />
          </div>
        )}

        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-sm font-medium">
            {loggedIn
              ? displayName
              : savedUsername || (t("settings.not_signed_in") || "Not signed in")}
          </span>
          <span className="truncate text-[11px] text-subtle">
            {loggedIn
              ? t("settings.signed_in_status") || "Signed in"
              : savedUsername
                ? t("settings.saved_not_signed_in") ||
                  "Saved account — not signed in"
                : t("settings.no_account_hint") ||
                  "Sign in to browse age-gated items and download"}
          </span>
        </div>

        {import.meta.env.DEV && (
          <div className="flex gap-1.5">
            <button className="btn-outline" onClick={openParser} disabled={!inTauri}>
              {t("settings.open_parser") || "Open Parser"}
            </button>
            <button className="btn-outline" onClick={onOpenParser} disabled={!inTauri}>
              {t("settings.parser_logs") || "Parser Logs"}
            </button>
          </div>
        )}
      </div>

      <div className="flex items-center gap-2">
        <button
          className="btn-primary inline-flex items-center gap-1.5"
          onClick={primaryAction}
          disabled={!inTauri || busy}
        >
          {busy ? (
            <span className="inline-block size-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
          ) : loggedIn ? (
            <LogOut className="size-4" />
          ) : (
            <LogIn className="size-4" />
          )}
          {loggedIn
            ? t("settings.sign_out") || "Sign out"
            : t("settings.sign_in") || "Sign in with Steam"}
        </button>

        {/* Only offered once a credential exists, and clearly destructive. */}
        {savedUsername && (
          <button
            className="btn-ghost text-error"
            onClick={forgetAccount}
            disabled={!inTauri}
            title={t("settings.remove_account") || "Remove saved account"}
          >
            {t("settings.forget_account") || "Forget"}
          </button>
        )}
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