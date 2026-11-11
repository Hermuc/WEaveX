import { useEffect, useState } from "react";
import { useTranslation } from "@/i18n/hooks";
import { openUrl as openExternal } from "@tauri-apps/plugin-opener";
import {
  BookOpen,
  ChevronDown,
  ChevronUp,
  FolderOpen,
  HelpCircle,
  Info,
  RefreshCw,
} from "lucide-react";

const Github = (props: React.SVGProps<SVGSVGElement>) => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    width="24"
    height="24"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    {...props}
  >
    <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
    <path d="M9 18c-4.51 2-5-2-7-2" />
  </svg>
);

import Dialog from "@/components/common/Dialog";
import { inTauri, invoke, tryInvoke } from "@/lib/tauri";
import AppIcon from "@/components/common/AppIcon";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCheckUpdates?: () => void;
  onOpenLegal?: () => void;
}

function FaqItem({ q, a }: { q: string; a: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="border-b border-white/5 last:border-0">
      <button 
        type="button" 
        className="flex w-full items-center justify-between py-2.5 text-left text-sm font-medium transition-colors hover:text-primary"
        onClick={() => setOpen(!open)}
      >
        <span className="pr-4">{q}</span>
        {open ? <ChevronUp className="size-4 shrink-0 opacity-50" /> : <ChevronDown className="size-4 shrink-0 opacity-50" />}
      </button>
      {open && (
        <div className="pb-3 text-xs leading-relaxed whitespace-pre-wrap text-muted">
          {a}
        </div>
      )}
    </div>
  );
}


export default function InfoDialog({
  open,
  onOpenChange,
  onCheckUpdates,
  onOpenLegal,
}: Props) {
  const { t } = useTranslation();
  const [version, setVersion] = useState<string>(__APP_VERSION__);
  const [showFaq, setShowFaq] = useState(false);

  useEffect(() => {
    if (!open || !inTauri) return;
    void tryInvoke<{ version: string; name: string }>("app_get_info").then(
      (v) => {
        if (v?.version) setVersion(v.version);
      },
    );
  }, [open]);

  const [prevOpen, setPrevOpen] = useState(open);
  if (open !== prevOpen) {
    setPrevOpen(open);
    if (!open) {
      setShowFaq(false);
    }
  }

  const toggleFaq = () => {
    setShowFaq(!showFaq);
  };

  const openLink = async (url: string) => {
    if (inTauri) await openExternal(url);
    else window.open(url, "_blank");
  };

  const openDataFolder = async () => {
    if (!inTauri) return;
    await invoke("app_open_data_dir").catch(() => undefined);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={
        <div className="flex items-center gap-2">
          <Info size={18} className="text-primary" />
          <span>{t("dialog.about")}</span>
        </div>
      }
      size="sm"
    >
      <div className="flex flex-col items-center gap-3 text-center">
        <AppIcon className="size-32" />
        <div className="space-y-1">
          <div className="text-lg font-semibold">{t("info.app_full_name")}</div>
          <div className="text-xs text-muted">
            {t("info.version_label")}{" "}
            <span className="text-foreground">{version || "—"}</span>
          </div>
        </div>
        <p className="text-sm text-muted">{t("info.description")}</p>
        <p className="text-xs text-subtle">{t("info.developed")}</p>

        <div className="flex flex-wrap items-center justify-center gap-2 pt-1">
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={() => openLink("https://github.com/psyattack/WEave")}
          >
            <Github className="size-4" />
            {t("buttons.github")}
          </button>
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={() => {
              if (onCheckUpdates) {
                onOpenChange(false);
                onCheckUpdates();
              }
            }}
          >
            <RefreshCw className="size-4" />
            {t("buttons.check_updates")}
          </button>
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={openDataFolder}
            disabled={!inTauri}
          >
            <FolderOpen className="size-4" />
            {t("buttons.open_data_folder")}
          </button>
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={() => openLink("https://github.com/psyattack/WEave/releases")}
          >
            <BookOpen className="size-4" />
            {t("buttons.changelog") || "Changelog"}
          </button>
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={toggleFaq}
            aria-expanded={showFaq}
          >
            <HelpCircle className="size-4" />
            {t("buttons.faq") || "FAQ"}
            {showFaq ? (
              <ChevronUp className="size-3.5" />
            ) : (
              <ChevronDown className="size-3.5" />
            )}
          </button>
          <button
            className="hover-shimmer flex items-center gap-2 rounded-md border border-white/10 bg-white/5 px-3 py-1.5 text-sm backdrop-blur-md transition-all hover:bg-white/10"
            onClick={() => {
              if (onOpenLegal) {
                onOpenChange(false);
                onOpenLegal();
              }
            }}
          >
            <BookOpen className="size-4" />
            {t("buttons.legal")}
          </button>
        </div>

        {showFaq && (
          <div className="w-full rounded-md border border-border bg-surface-sunken p-3 text-left">
            <div className="mb-2 text-[11px] font-semibold tracking-wide text-subtle uppercase">
              {t("info.faq_title") || "Frequently Asked Questions"}
            </div>
            <div className="flex flex-col">
              <FaqItem 
                q={t("faq.download_issues_q") || "What to do if wallpapers will not download?"} 
                a={t("faq.download_issues_a") || "Try again, maybe wait a little.\nIt is also possible that the passwords for the system download accounts have expired. In that case you can:\n1) Add your own account with a copy of Wallpaper Engine in the settings\n2) Buy an offline activation of Wallpaper Engine and add it the same way\n3) Open an Issue on GitHub and I will try to update the working accounts"} 
              />
              <FaqItem 
                q={t("faq.login_issues_q") || "Wallpapers do not load in the app, or a Steam login error appears"} 
                a={t("faq.login_issues_a") || "If you do not need 18+ content you can ignore this.\n\nOtherwise, open the data folder (the 'Open Data Folder' button in the About window) and delete the 'EBWebView' folder (this clears the embedded browser cache). Restart the app.\n\nIf the problem persists, it is most likely an issue with the system parser account; try setting your own Steam account in the parser settings (Accounts -> Custom account). Any account works as long as it does not have the 18+ content restriction enabled."} 
              />
              <FaqItem 
                q={t("faq.extract_q") || "Why does the 'Extract' button not work for some downloaded wallpapers?"} 
                a={t("faq.extract_a") || "Not every Workshop item is a full project (with a project.json file). There are video wallpapers (MP4) and web wallpapers (HTML). They are already usable and cannot (and need not) be unpacked as Wallpaper Engine projects."} 
              />
              <FaqItem 
                q={t("faq.storage_q") || "Where are downloaded wallpapers stored?"} 
                a={t("faq.storage_a") || "All downloaded wallpapers are saved directly to the root Wallpaper Engine folder, in the 'projects/myprojects' directory. They become available in the official app immediately."} 
              />
              <FaqItem 
                q={t("faq.visibility_q") || "Wallpapers download but are not visible in the original app"} 
                a={t("faq.visibility_a") || "Make sure the Settings point at the correct Wallpaper Engine folder. The path must point at the root folder that contains wallpaper32.exe / wallpaper64.exe."} 
              />
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}
