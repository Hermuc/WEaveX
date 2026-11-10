import { useEffect, useState } from "react";
import { useTranslation } from "@/i18n/hooks";
import { CheckCircle2, ExternalLink, Loader2, RefreshCw, Sparkles } from "lucide-react";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";

import Dialog from "@/components/common/Dialog";
import { inTauri, invoke, tryInvoke } from "@/lib/tauri";
import { useUpdaterStore } from "@/stores/updater";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

interface UpdateInfo {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_notes: string;
  html_url: string;
  error?: string | null;
}

export default function UpdateDialog({ open, onOpenChange }: Props) {
  const { t } = useTranslation();
  const cachedInfo = useUpdaterStore((s) => s.info);
  const [info, setInfo] = useState<UpdateInfo | null>(cachedInfo);
  const [loading, setLoading] = useState(false);

  const run = async () => {
    setLoading(true);
    if (inTauri) {
      const data = await tryInvoke<UpdateInfo>("updater_check");
      setInfo(data ?? null);
      if (data) useUpdaterStore.getState().show(data);
    } else {
      setInfo({
        current_version: __APP_VERSION__,
        latest_version: __APP_VERSION__,
        update_available: false,
        release_notes: "(mock) You are up to date.",
        html_url: "",
      });
    }
    setLoading(false);
  };

  useEffect(() => {
    if (!open) return;
    const timer = setTimeout(() => {
      void run();
      (document.activeElement as HTMLElement)?.blur();
    }, 0);
    return () => clearTimeout(timer);
  }, [open]);

  const skipThisVersion = async () => {
    if (!info?.latest_version || !inTauri) return;
    await invoke("updater_skip_version", {
      version: info.latest_version,
    }).catch(() => undefined);
    useUpdaterStore.getState().dismiss();
    onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={
        <div className="flex items-center gap-2">
          <RefreshCw size={18} className="text-primary" />
          <span>{t("buttons.check_updates")}</span>
        </div>
      }
      size="sm"
    >
      <div className="space-y-3">
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-muted">
            <Loader2 className="size-4 animate-spin" />
            {t("labels.loading_dots")}
          </div>
        ) : info ? (
          <>
            <div className="flex items-center justify-between rounded-md border border-border bg-surface-sunken p-3 text-sm">
              <span className="text-muted">{t("labels.current_version")}</span>
              <span>{info.current_version}</span>
            </div>
            {info.update_available ? (
              <div className="card border-primary/40 bg-primary/10 p-3 text-sm">
                <p className="mb-2 flex items-center gap-2 font-medium">
                  <Sparkles className="size-4 shrink-0 text-primary" />
                  <span>
                    {t("labels.update_available_for", {
                      version: info.latest_version,
                    })}
                  </span>
                </p>
                <div className="mt-3 flex gap-2">
                  <button
                    className="btn-primary flex flex-1 items-center justify-center gap-2"
                    onClick={() => {
                      const url =
                        info.html_url ||
                        "https://github.com/psyattack/WEave/releases";
                      if (inTauri) openExternal(url);
                    }}
                  >
                    <ExternalLink className="size-4" />
                    <span>{t("buttons.download_release")}</span>
                  </button>
                  <button className="btn-ghost" onClick={skipThisVersion}>
                    {t("buttons.skip_version") || "Skip this version"}
                  </button>
                </div>
              </div>
            ) : (
              <div className="flex items-center gap-2 rounded-md border border-success/40 bg-success/10 p-3 text-sm text-success">
                <CheckCircle2 className="size-4" />
                {t("labels.up_to_date")}
              </div>
            )}
            {info.error && <p className="text-xs text-danger">{info.error}</p>}
            <button className="btn-ghost w-full" onClick={run}>
              <RefreshCw className="size-4" />
              {t("tooltips.refresh")}
            </button>
          </>
        ) : null}
      </div>
    </Dialog>
  );
}
