import { useState, useEffect, useRef, useCallback } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { Lock, User, KeyRound } from "lucide-react";
import { useTranslation } from "@/i18n/hooks";
import { invoke, tryInvoke, tryInvokeAction, tryInvokeOk } from "@/lib/tauri";
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
  // Which Steam Guard kinds the current challenge accepts ("device_code" /
  // "email_code"), so the submit button can tell Steam the right type.
  const [guardKinds, setGuardKinds] = useState<string[]>([]);
  const [guardCode, setGuardCode] = useState("");
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

  /// Refresh the sidebar status from the authoritative probe.
  ///
  /// `unknown` (the network call failed) must leave the badge alone rather than
  /// clearing it: treating a timeout as "signed out" is what made the status
  /// flicker between signed-in and failed.
  const refreshSession = useCallback(async () => {
    const probe = await tryInvoke<{
      state: string;
      account?: SteamAccountInfo;
    }>("steam_probe_account", undefined, { state: "unknown" });

    if (probe?.state === "signed_in") {
      setSessionLoggedIn(probe.account ?? null);
    } else if (probe?.state === "signed_out") {
      setSessionLoggedIn(null);
    }
    // `unknown`: keep whatever the sidebar already shows.
  }, [setSessionLoggedIn]);

  const finalizeLogin = useCallback(async () => {
    await invoke<number>("steam_sync_cookies").catch(() => 0);

    // Persist the credentials we just signed in with.
    //
    // Without this the sign-in only ever lived in the WebView2 cookie jar, and
    // `steamLoginSecure` is a *session* cookie — it dies with the process. The
    // next launch had no stored account to auto-login with (`user_accounts.enc`
    // still held whatever was saved days earlier, or nothing), so the app always
    // reported "login failed" on startup even though the previous session had
    // been perfectly fine.
    //
    // `accounts_set_custom` is idempotent: an existing entry is simply updated,
    // so re-logging in with a new password refreshes it.
    const user = username.trim();
    const pass = password;
    if (user && pass) {
      const saved = await tryInvokeOk("accounts_set_custom", {
        username: user,
        password: pass,
      });
      if (!saved) {
        // Non-fatal: the session in the webview is already valid. Only the
        // auto-login-on-next-start convenience is lost.
        console.warn("[login] could not persist credentials for", user);
      }
    }

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
  }, [refreshSession, setAccounts, onOpenChange, t, username, password]);

  useEffect(() => {
    let interval: ReturnType<typeof setInterval>;
    if (open && isPrepared) {
      interval = setInterval(async () => {
        // Use the same three-state probe as the startup path.
        //
        // `steam_is_logged_in` only checks whether a `steamLoginSecure` cookie
        // exists, so it can say yes while Steam has already invalidated the
        // session — and the startup path can say no for the opposite reason.
        // Mixing the two produced exactly the flip-flopping badge users saw:
        // "signed in" and "login failed" alternating without any action.
        const probe = await tryInvoke<{ state: string }>(
          "steam_probe_account",
          undefined,
          { state: "unknown" },
        );
        if (probe?.state === "signed_in") {
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


  /// Submit a Steam Guard code typed by the user.
  const handleGuardSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!guardCode.trim()) return;

    setErrorMsg(null);
    setBusy(true);
    try {
      await invoke<string>("steam_oauth_submit_guard", {
        code: guardCode.trim(),
        kind: guardKinds[0] ?? null,
      });
      await finalizeLogin();
    } catch (e) {
      const msg = String(e);
      setErrorMsg(
        msg.includes("INVALID_GUARD_CODE")
          ? "That Steam Guard code was not accepted. Try again."
          : msg,
      );
    } finally {
      setBusy(false);
    }
  };

  const handleCredentialsSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!username || !password) return;

    setErrorMsg(null);
    setBusy(true);
    busyRef.current = true;
    try {
      // Sign in through Steam's OAuth API rather than by filling the web form.
      //
      // Filling the form always ends at a Steam Guard page the headless webview
      // cannot answer. The OAuth API instead *tells us* which guard Steam wants,
      // so the user can type the code here and we submit it directly.
      const res = await invoke<{
        status: "approved" | "guard_code" | "mobile_confirm";
        allowed_kinds: string[];
        steam_id: string | null;
      }>("steam_oauth_begin", { username, password });

      if (res.status === "approved") {
        await finalizeLogin();
        return;
      }

      if (res.status === "guard_code") {
        // Ask the user for the code; `handleGuardSubmit` finishes the flow.
        setGuardKinds(res.allowed_kinds);
        setStep("2fa");
        return;
      }

      // mobile_confirm is resolved inside the command; anything else is a bug.
      setErrorMsg("Unexpected Steam response. Try again.");
    } catch (e) {
      const msg = String(e);
      setErrorMsg(
        msg.includes("INVALID_PASSWORD")
          ? "Incorrect username or password."
          : msg,
      );
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
                  "填写你自己的 Steam 账号，或直接点「重新登录」使用你在「账户」里已添加的账号。"}
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
                      {t("settings.relogin") || "Relogin"}
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
              <form onSubmit={handleGuardSubmit} className="flex flex-col gap-5 py-2">
                {errorMsg && (
                  <div className="flex items-center gap-3 rounded-lg bg-red-500/10 p-3 text-red-500">
                    <p className="text-xs">{errorMsg}</p>
                  </div>
                )}

                <div className="flex flex-col gap-2">
                  <label className="text-sm font-medium">
                    {guardKinds[0] === "email_code"
                      ? "Steam 邮件验证码"
                      : "Steam Guard 验证码"}
                  </label>
                  <input
                    autoFocus
                    className="input w-full text-center font-mono tracking-widest"
                    placeholder={
                      guardKinds[0] === "email_code" ? "ABCDE" : "ABCDE"
                    }
                    value={guardCode}
                    onChange={(e) => setGuardCode(e.target.value.toUpperCase())}
                    disabled={busy}
                    maxLength={10}
                  />
                  <p className="text-[11px] leading-relaxed text-muted">
                    {guardKinds[0] === "email_code"
                      ? "Steam 已把验证码发到你的邮箱，请填入上面的输入框。"
                      : "请打开 Steam 手机 App 的「Steam 令牌」，输入其中的验证码。"}
                  </p>
                </div>

                <div className="flex justify-end gap-2">
                  <button
                    type="button"
                    className="btn-ghost"
                    onClick={() => {
                      setStep("credentials");
                      setErrorMsg(null);
                      setGuardCode("");
                    }}
                    disabled={busy}
                  >
                    {"返回"}
                  </button>
                  <button
                    type="submit"
                    className="btn-primary flex items-center gap-2"
                    disabled={busy || !guardCode.trim()}
                  >
                    {busy && (
                      <span className="inline-block size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" />
                    )}
                    {"验证"}
                  </button>
                </div>
              </form>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}