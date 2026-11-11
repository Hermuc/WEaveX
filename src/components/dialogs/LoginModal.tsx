import { useState, useEffect, useRef, useCallback } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { Lock, User, KeyRound } from "lucide-react";
import { useTranslation } from "@/i18n/hooks";
import { invoke, tryInvoke, tryInvokeAction } from "@/lib/tauri";
import { pushToast } from "@/stores/toasts";
import { useSteamSessionStore, SteamAccountInfo } from "@/stores/steam-session";
import { useAppStore } from "@/stores/app";
import { triggerGlobalRefresh } from "@/stores/refresh";

export default function LoginModal({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const [step, setStep] = useState<"credentials" | "2fa">("credentials");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [qrCode, setQrCode] = useState<string | null>(null);
  const [qrSession, setQrSession] = useState<{
    clientId: string;
    requestId: string;
    createdAt: number;
  } | null>(null);
  const [isPrepared, setIsPrepared] = useState(false);
  // When true, the real Steam window has been shown so the user can finish
  // Steam Guard there; we only poll for the resulting session.

  const setShowLoginPromptOnFail = useAppStore(
    (s) => s.setShowLoginPromptOnFail,
  );
  const loginModalMode = useAppStore((s) => s.loginModalMode);
  const setAccounts = useAppStore((s) => s.setAccounts);
  const setSessionLoggedIn = useSteamSessionStore((s) => s.setLoggedIn);

  const [prevOpen, setPrevOpen] = useState(open);
  if (open !== prevOpen) {
    setPrevOpen(open);
    if (open) {
      setStep("credentials");
      setUsername("");
      setPassword("");
      setBusy(false);
      setErrorMsg(null);
      setQrCode(null);
      setQrSession(null);
      setIsPrepared(false);
    } else {
      setIsPrepared(false);
      setQrSession(null);
    }
  }

  // Reset state when opened
  useEffect(() => {
    if (!open) return;

    tryInvoke("steam_login_prepare").then(() => {
      setIsPrepared(true);
    });

    // Init QR Session. The backend renders the challenge locally and hands back
    // a data: URL — pointing <img> at an external QR service used to fail (our
    // CSP img-src blocked it, and the host is unreliable from CN).
    tryInvoke<{
      client_id: string;
      challenge_url: string;
      request_id: string;
      qr_data_url: string;
    }>("steam_qr_begin").then((res) => {
      if (res) {
        setQrSession({
          clientId: res.client_id,
          requestId: res.request_id,
          createdAt: Date.now(),
        });
        setQrCode(res.qr_data_url || null);
      }
    });
  }, [open]);

  const refreshSession = useCallback(async () => {
    const v = await tryInvoke<boolean>("steam_is_logged_in", undefined, false);
    if (!v) {
      await tryInvoke("steam_current_account", undefined);
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

  const finalizeLogin = useCallback(async () => {
    await invoke<number>("steam_sync_cookies").catch(() => 0);
    pushToast(
      t("messages.signed_in_to_steam") || "Signed in to Steam",
      "success",
    );
    const list = await tryInvoke<
      { index: number; username: string; is_custom: boolean }[]
    >("accounts_list", undefined, []);
    if (list) setAccounts(list);
    await refreshSession();
    triggerGlobalRefresh();
    onOpenChange(false);
  }, [refreshSession, setAccounts, onOpenChange, t]);

  useEffect(() => {
    let interval: ReturnType<typeof setInterval>;
    if (open && isPrepared) {
      interval = setInterval(async () => {
        const isLoggedIn = await tryInvoke<boolean>(
          "steam_is_logged_in",
          undefined,
          false,
        );
        if (isLoggedIn) {
          clearInterval(interval);
          await finalizeLogin();
          return;
        }

        if (step === "credentials" && qrSession) {
          if (Date.now() - qrSession.createdAt > 25000) {
            tryInvoke<{
              client_id: string;
              challenge_url: string;
              request_id: string;
              qr_data_url: string;
            }>("steam_qr_begin").then((res) => {
              if (res) {
                setQrSession({
                  clientId: res.client_id,
                  requestId: res.request_id,
                  createdAt: Date.now(),
                });
                setQrCode(res.qr_data_url || null);
              }
            });
            return;
          }

          const res = await tryInvoke<any>("steam_qr_poll", {
            clientId: qrSession.clientId,
            requestId: qrSession.requestId,
          });
          if (res && res.response) {
            if (res.response.refresh_token && res.response.access_token) {
              clearInterval(interval);
              setBusy(true);
              try {
                await tryInvoke("steam_qr_login_finalize", {
                  accessToken: res.response.access_token,
                  refreshToken: res.response.refresh_token,
                });
                await finalizeLogin();
              } catch (e: any) {
                setErrorMsg(String(e));
                setBusy(false);
                busyRef.current = false;
              }
            }
          }
        }

        if (busyRef.current) {
          const err = await tryInvoke<string | null>(
            "steam_login_poll_error",
            undefined,
            null,
          );
          if (err && err.length > 20) {
            setErrorMsg(err);
            setBusy(false);
            busyRef.current = false;
          }
        }
      }, 3500);
    }
    return () => clearInterval(interval);
  }, [open, step, qrSession, isPrepared, finalizeLogin]);

  const handleSystemRelogin = async () => {
    setBusy(true);
    busyRef.current = true;
    try {
      const res = await tryInvokeAction<boolean>("steam_auto_login", {
        accountIndex: null,
        force: true,
      });
      if (res.ok && res.value) {
        const list = await tryInvoke<
          { index: number; username: string; is_custom: boolean }[]
        >("accounts_list", undefined, []);
        if (list) setAccounts(list);
        await refreshSession();
        triggerGlobalRefresh();
        onOpenChange(false);
      } else {
        const error = !res.ok ? res.error : "Unknown error";
        pushToast(`${t("messages.error") || "Relogin failed"}: ${error}`, "error");
      }
    } finally {
      if (open) {
        setBusy(false);
        busyRef.current = false;
      }
    }
  };

  // We no longer try to drive Steam Guard ourselves. The only reliable path
  // is: submit the credentials in the real Steam window, then let *Steam*
  // handle the guard prompt. This poller simply waits for the session cookie
  // that appears once the user has finished (up to 5 minutes).
  const waitForLoginResult = async (maxAttempts = 300) => {
    for (let i = 0; i < maxAttempts; i++) {
      await new Promise((r) => setTimeout(r, 1000));
      const isLoggedIn = await tryInvoke<boolean>(
        "steam_is_logged_in",
        undefined,
        false,
      );
      if (isLoggedIn) return "SUCCESS";
      if (!busyRef.current) return "CANCELLED";
      if (i >= 5) {
        const polled = await tryInvoke<string | null>(
          "steam_login_poll_error",
          undefined,
          null,
        );
        // Only a real credential error ends the wait; a Steam Guard screen is
        // not an error and is handled in the Steam window by the user.
        if (polled && polled.length > 20 && /密码|password|incorrect|invalid|verify|captcha|次数|尝试/i.test(polled)) {
          setErrorMsg(polled);
          return "ERROR";
        }
      }
    }
    return "FAILED";
  };

  const handleCredentialsSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username || !password) return;

    setErrorMsg(null);
    setBusy(true);
    busyRef.current = true;
    try {
      // Hand the credentials to Steam and open its real login window.
      //
      // The backend shows the window first (WebView2 suspends scripts while
      // hidden), drops any stale session, opens the credential form and
      // *best-effort* prefills it. It returns true only when a session cookie
      // already exists — i.e. no Steam Guard was needed.
      const loggedIn = await invoke<boolean>("steam_login_fill", {
        username,
        password,
      });
      if (loggedIn) {
        await finalizeLogin();
        return;
      }
      // Steam Guard (mobile confirmation / email code / new device) is Steam's
      // own flow. The window is already on screen — the user finishes there.
      setStep("2fa");
      const result = await waitForLoginResult(300);
      if (result === "SUCCESS") {
        await invoke("steam_login_hide").catch(() => undefined);
        await finalizeLogin();
      } else if (result === "ERROR") {
        await invoke("steam_login_hide").catch(() => undefined);
        setStep("credentials");
      } else if (result === "FAILED") {
        setErrorMsg(
          "Timed out waiting for Steam. Finish signing in inside the Steam window and try again.",
        );
      }
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setBusy(false);
      busyRef.current = false;
    }
  };


  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-background/80 backdrop-blur-sm data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <Dialog.Content className="fixed top-[50%] left-[50%] z-50 grid w-full max-w-sm translate-[-50%] gap-4 overflow-hidden border border-border bg-surface shadow-xl duration-200 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-left-1/2 data-[state=closed]:slide-out-to-top-[48%] data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:slide-in-from-left-1/2 data-[state=open]:slide-in-from-top-[48%] data-[state=open]:zoom-in-95 sm:rounded-xl">
          <div className="flex flex-col space-y-1.5 p-6 pb-4">
            <Dialog.Title className="flex items-center gap-2 text-xl leading-none font-semibold tracking-tight">
              <Lock className="size-5 text-primary" />
              {t("settings.login_title") || "Steam Login"}
            </Dialog.Title>
            <Dialog.Description className="text-sm text-muted">
              {step === "credentials"
                ? t("settings.login_description") ||
                  "Enter your Steam account credentials to log in to the parser without opening the browser window."
                : t("settings.login_2fa_description") ||
                  "Enter your Steam Guard code or confirm the login on your mobile device."}
            </Dialog.Description>
            {step === "credentials" && (
              <p className="pt-1 text-[11px] leading-relaxed text-subtle/80">
                {t("settings.login_account_hint") ||
                  "填写你自己的 Steam 账号即可；留空直接点「重新登录系统账户」时，会优先使用你在「账户」里添加的自定义账号。"}
              </p>
            )}
          </div>

          <div className="px-6 pb-6">
            {!isPrepared ? (
              <div className="flex flex-col items-center justify-center gap-3 py-10">
                <span className="inline-block size-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                <span className="text-sm text-muted">
                  {t("labels.loading_dots") || "Loading..."}
                </span>
              </div>
            ) : step === "credentials" ? (
              <div className="flex flex-col gap-6">
                {errorMsg && (
                  <div className="mb-0 flex items-center gap-3 rounded-lg bg-red-500/10 p-3 text-red-500">
                    <p className="text-xs">{errorMsg}</p>
                  </div>
                )}
                <form onSubmit={handleCredentialsSubmit} className="space-y-4">
                  <div className="space-y-2">
                    <div className="relative">
                      <User className="absolute top-2.5 left-3 size-4 text-muted" />
                      <input
                        autoFocus
                        className="input w-full pl-9"
                        placeholder={t("settings.username") || "Username"}
                        value={username}
                        onChange={(e) => setUsername(e.target.value)}
                        disabled={busy}
                      />
                    </div>
                    <div className="relative">
                      <KeyRound className="absolute top-2.5 left-3 size-4 text-muted" />
                      <input
                        type="password"
                        className="input w-full pl-9"
                        placeholder={t("settings.password") || "Password"}
                        value={password}
                        onChange={(e) => setPassword(e.target.value)}
                        disabled={busy}
                      />
                    </div>
                  </div>

                  <div className="flex w-full items-center justify-between gap-3 pt-4">
                    <button
                      type="button"
                      className="btn-secondary px-3 text-xs"
                      onClick={handleSystemRelogin}
                      disabled={busy}
                    >
                      {busy && (
                        <span className="mr-1 inline-block size-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
                      )}
                      {t("settings.relogin") || "Relogin to system account"}
                    </button>
                    <div className="flex gap-2">
                      <button
                        type="button"
                        className="btn-ghost"
                        onClick={() => onOpenChange(false)}
                        disabled={busy}
                      >
                        {t("buttons.cancel") || "Cancel"}
                      </button>
                      <button
                        type="submit"
                        className="btn-primary flex items-center gap-2"
                        disabled={busy || !username || !password}
                      >
                        {busy && (
                          <span className="inline-block size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                        )}
                        {t("settings.login_button") || "Login"}
                      </button>
                    </div>
                  </div>
                  {loginModalMode === "auto" && (
                    <div className="mt-5 mb-1 flex justify-center">
                      <button
                        type="button"
                        className="text-[11px] font-medium tracking-wide text-muted uppercase opacity-60 transition-colors hover:text-primary hover:opacity-100"
                        onClick={() => {
                          setShowLoginPromptOnFail(false);
                          onOpenChange(false);
                        }}
                        disabled={busy}
                      >
                        {t("settings.continue_offline") ||
                          "Continue without account (Do not ask again)"}
                      </button>
                    </div>
                  )}
                </form>
                {qrCode && (
                  <>
                    <div className="h-px w-full bg-border" />
                    <div className="flex flex-col items-center justify-center gap-3">
                      <div className="rounded-xl bg-white p-2.5 shadow-sm">
                        <img
                          src={qrCode}
                          alt="Steam QR Login"
                          className="size-40 object-contain"
                        />
                      </div>
                      <span className="max-w-50 text-center text-xs font-medium text-muted">
                        {t("settings.steam_qr_help") ||
                          "Use the Steam Mobile App to sign in via QR code"}
                      </span>
                    </div>
                  </>
                )}
              </div>
            ) : (
              <div className="flex flex-col gap-5 py-2">
                {errorMsg && (
                  <div className="flex items-center gap-3 rounded-lg bg-red-500/10 p-3 text-red-500">
                    <p className="text-xs">{errorMsg}</p>
                  </div>
                )}

                <div className="flex flex-col items-center justify-center gap-3 rounded-xl border border-primary/10 bg-primary/5 p-5 text-center">
                  <span className="inline-block size-6 animate-spin rounded-full border-2 border-primary border-t-transparent" />
                  <p className="text-sm font-medium">
                    {t("settings.steam_window_title") ||
                      "Steam 登录窗口已打开，请在其中完成登录"}
                  </p>
                  <p className="text-xs leading-relaxed text-muted">
                    {t("settings.steam_window_help") ||
                      "如果 Steam 要求手机确认、邮箱验证码或设备授权，请直接在那个窗口里操作。完成后本窗口会自动关闭。"}
                  </p>
                </div>

                <div className="flex flex-col gap-2">
                  <button
                    type="button"
                    className="btn-secondary w-full py-2 text-sm"
                    onClick={async () => {
                      await invoke("steam_login_reveal").catch(() => undefined);
                    }}
                  >
                    {t("settings.show_steam_window") || "重新显示 Steam 窗口"}
                  </button>
                  <button
                    type="button"
                    className="btn-ghost w-full py-2 text-sm"
                    onClick={() => {
                      busyRef.current = false;
                      void invoke("steam_login_hide").catch(() => undefined);
                      setStep("credentials");
                      setErrorMsg(null);
                      setBusy(false);
                    }}
                  >
                    {t("buttons.cancel") || "取消"}
                  </button>
                </div>
              </div>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}