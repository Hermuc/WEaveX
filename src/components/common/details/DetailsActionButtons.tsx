import { useState, useEffect, useCallback } from "react";
import {
  Clapperboard,
  Copy,
  Download,
  ExternalLink,
  FolderOpen,
  Package,
  Play,
  Trash2,
  MoreHorizontal,
  Settings2,
} from "lucide-react";
import * as DropdownMenu from "@radix-ui/react-dropdown-menu";
import { open as openPath } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "@/i18n/hooks";
import { inTauri, tryInvoke, tryInvokeOk, tryInvokeAction } from "@/lib/tauri";
import { pushToast } from "@/stores/toasts";
import { maybeMinimize } from "@/lib/window";
import { useConfirm } from "@/hooks/useConfirm";
import { Tooltip } from "@/components/common/Tooltip";
import { Meta } from "@/hooks/useDetailsMeta";
import { InstalledWallpaper, WorkshopItem } from "@/types/workshop";

interface DetailsActionButtonsProps {
  kind: "workshop" | "installed";
  item: WorkshopItem | InstalledWallpaper | null;
  meta: Meta;
  installedEntry: InstalledWallpaper | undefined;
  showInstalledActions: boolean;
  openWorkshopPage: () => Promise<void>;
  refreshInstalled: () => Promise<void>;
  removeOptimistic: (pubfileid: string) => void;
  addOptimistic: (pubfileid: string, wallpaper: InstalledWallpaper) => void;
  onClose: () => void;
  onDownload?: (item: WorkshopItem) => void;
  /// Stream the wallpaper's video to an external player while it downloads.
  /// Only offered for video wallpapers; undefined hides the button.
  onWatch?: (item: WorkshopItem) => void;
  /// Progress/error state for the watch button, so it can show a spinner.
  /// Toasts are suppressed while this panel is open, so the button is the only
  /// place feedback can appear.
  watchState?: { pendingId: string | null; error: string | null };
  onApply?: (item: InstalledWallpaper) => void;
  onExtract?: (item: InstalledWallpaper) => void;
  onDelete?: (item: InstalledWallpaper) => void;
  onOpenFolder?: (item: InstalledWallpaper) => void;
  onToggleSettings?: () => void;
}

async function pickDir(): Promise<string | null> {
  if (!inTauri) return null;
  const r = await openPath({ directory: true });
  if (!r || Array.isArray(r)) return null;
  return r;
}

export default function DetailsActionButtons({
  kind,
  item,
  meta,
  installedEntry,
  showInstalledActions,
  openWorkshopPage,
  refreshInstalled,
  removeOptimistic,
  addOptimistic,
  onClose,
  onDownload,
  onWatch,
  watchState,
  onApply,
  onExtract,
  onDelete,
  onOpenFolder,
  onToggleSettings,
}: DetailsActionButtonsProps) {
  const { t } = useTranslation();
  const { confirm, ConfirmDialog } = useConfirm();

  const installedHandle: InstalledWallpaper | null =
    kind === "installed"
      ? (item as InstalledWallpaper | null)
      : (installedEntry ?? null);

  const [isActive, setIsActive] = useState(false);

  // Depend on the id, not on the object: `installedHandle` is rebuilt on
  // every render (the ternary hands back a fresh reference whenever the
  // parent re-renders with a new object), so listing it as a dependency
  // made `checkActive` — and therefore the polling `useEffect` below —
  // tear down and re-create itself on every render. The interval never
  // survived long enough to fire, so `isActive` stayed at its initial
  // `false` and the preset-settings button was permanently disabled.
  const activeTargetId = installedHandle?.pubfileid ?? null;

  const checkActive = useCallback(async () => {
    if (!activeTargetId || !inTauri) {
      setIsActive(false);
      return;
    }
    const activeIds = await tryInvoke<string[]>("we_active_pubfileids", undefined, []) || [];
    setIsActive(activeIds.includes(activeTargetId));
  }, [activeTargetId]);

  useEffect(() => {
    let cancelled = false;
    const run = async () => {
      await Promise.resolve();
      if (!cancelled) {
        void checkActive();
      }
    };
    void run();

    const interval = setInterval(() => {
      void checkActive();
    }, 2000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [checkActive]);

  const overlayApply = async () => {
    if (!installedHandle) return;
    const res = await tryInvokeAction("we_apply", {
      projectPath: installedHandle.project_json_path,
      monitor: null,
      force: false,
    });
    if (res.ok) {
      pushToast(t("messages.applied") || "Applied", "success");
      void maybeMinimize();
      void checkActive();
    } else {
      pushToast(`${t("messages.apply_failed") || "Apply failed"}: ${res.error}`, "error");
    }
  };

  const overlayExtract = async () => {
    if (!installedHandle) return;
    if (!installedHandle.has_pkg) {
      pushToast(t("messages.no_pkg_file") || "No .pkg file", "warning");
      return;
    }
    const folder = await pickDir();
    if (!folder) return;
    const res = await tryInvokeAction("extract_start", {
      pubfileid: installedHandle.pubfileid,
      outputDir: folder,
    });
    pushToast(
      res.ok
        ? t("messages.extraction_started") || "Extract started"
        : `${t("messages.error") || "Extract failed"}: ${res.error}`,
      res.ok ? "success" : "error",
    );
  };

  const overlayOpenFolder = async () => {
    if (!installedHandle) return;
    await tryInvokeOk("open_path", { path: installedHandle.folder });
  };

  const overlayDelete = async () => {
    if (!installedHandle) return;
    if (inTauri) {
      const activeIds = await tryInvoke<string[]>("we_active_pubfileids", undefined, []) || [];
      if (activeIds.includes(installedHandle.pubfileid)) {
        pushToast(
          t("messages.cannot_delete_active_single") ||
            "Wallpaper is currently active — switch first.",
          "error",
        );
        return;
      }
    }
    const confirmed = await confirm({
      title: t("tooltips.delete_wallpaper") || "Delete Wallpaper",
      message:
        t("messages.confirm_delete") ||
        "Delete this wallpaper?\n\nThe wallpaper folder will be removed from your Wallpaper Engine library permanently. This action cannot be undone.",
      confirmLabel: t("buttons.delete") || "Delete",
      cancelLabel: t("buttons.cancel") || "Cancel",
      variant: "danger",
    });
    if (!confirmed) return;

    removeOptimistic(installedHandle.pubfileid);
    onClose();

    const res = await tryInvokeAction("we_delete_wallpaper", {
      pubfileid: installedHandle.pubfileid,
    });

    if (res.ok) {
      pushToast(t("messages.deleted") || "Deleted", "success");
      void refreshInstalled();
    } else {
      addOptimistic(installedHandle.pubfileid, installedHandle);
      pushToast(
        `${t("messages.delete_failed") || "Delete failed"}: ${res.error}`,
        "error",
      );
    }
  };

  const copyIdHandle = installedHandle || (kind === "workshop" ? item : null);

  return (
    <>
      <div className="flex w-full items-center gap-1.5">
        {/* Primary Action */}
        <div className="flex-1">
          {showInstalledActions && installedHandle ? (
            <button
              onClick={async () => {
                if (kind === "installed" && onApply) {
                  await onApply(installedHandle);
                } else {
                  await overlayApply();
                }
                void checkActive();
              }}
              className="hover-shimmer flex w-full items-center justify-center gap-1.5 rounded-md bg-primary px-4 py-2 text-sm font-semibold text-primary-foreground shadow-lg transition-colors hover:bg-primary/90"
            >
              <Play className="size-4" />
              {t("tooltips.install_wallpaper")}
            </button>
          ) : kind === "workshop" ? (
            <button
              disabled={meta.is_collection}
              onClick={() => {
                if (item && onDownload) {
                  onDownload(item as WorkshopItem);
                  onClose();
                }
              }}
              className="hover-shimmer flex w-full items-center justify-center gap-1.5 rounded-md bg-primary px-4 py-2 text-sm font-semibold text-primary-foreground shadow-lg transition-colors hover:bg-primary/90 disabled:opacity-50"
            >
              <Download className="size-4" />
              {t("buttons.install")}
            </button>
          ) : (
            <div className="w-full" />
          )}
        </div>

        {/* Video wallpapers can be streamed to an external player while the
            download is still running, so the user does not wait for the whole
            file. Only shown when the caller supplies a handler. */}
        {kind === "workshop" && onWatch && (() => {
          const itemId = item ? (item as WorkshopItem).pubfileid : null;
          const busy = !!watchState?.pendingId && watchState.pendingId === itemId;
          return (
            <div className="flex flex-col gap-1.5">
              <button
                disabled={meta.is_collection || busy}
                onClick={() => {
                  // Do not close the panel: the button itself shows progress,
                  // and the user needs to see errors without reopening.
                  if (item && onWatch && !busy) onWatch(item as WorkshopItem);
                }}
                className="hover-shimmer flex w-full items-center justify-center gap-1.5 rounded-md border border-primary/25 bg-primary/10 px-4 py-2 text-sm font-semibold text-primary transition-colors hover:bg-primary/20 disabled:opacity-60"
              >
                {busy ? (
                  <>
                    <span className="inline-block size-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
                    {t("messages.streaming_starting")}
                  </>
                ) : (
                  <>
                    <Clapperboard className="size-4" />
                    {t("messages.watch_while_downloading")}
                  </>
                )}
              </button>
              {watchState?.error && (
                <p className="text-[11px] leading-relaxed text-red-500">
                  {watchState.error}
                </p>
              )}
            </div>
          );
        })()}

        {/* Moved Open WE to dropdown */}

        {/* Preset settings edit the wallpaper's project.json — they do not
            require the wallpaper to be on screen right now. Leaving this
            disabled for inactive wallpapers just looked like a broken button:
            users with several similar walls could not tell which one was
            actually applied. Keep it clickable and explain the consequence in
            the tooltip instead. */}
        {showInstalledActions && installedHandle && (
          <Tooltip
            content={
              // Both keys exist in the locale files, so they are type-checked
              // now: a typo here is a compile error instead of a raw key
              // rendered into the tooltip (which is how
              // `tooltips.preset_settings_inactive` leaked to users).
              isActive
                ? t("tooltips.preset_settings")
                : t("tooltips.preset_settings_inactive")
            }
            side="top"
          >
            <button
              onClick={onToggleSettings}
              aria-label="Preset Settings"
              className={`hover-shimmer flex size-9 items-center justify-center rounded-md bg-white/5 text-foreground transition-colors hover:bg-white/10 ${isActive ? "" : "opacity-70"}`}
            >
              <Settings2 className="size-4" />
            </button>
          </Tooltip>
        )}

        {showInstalledActions && installedHandle && (
          <Tooltip content={t("tooltips.extract_wallpaper")} side="top">
            <button
              disabled={!meta.has_pkg}
              onClick={() =>
                kind === "installed" && onExtract
                  ? onExtract(installedHandle)
                  : void overlayExtract()
              }
              aria-label="Extract"
              className="hover-shimmer flex size-9 items-center justify-center rounded-md bg-white/5 text-foreground transition-colors hover:bg-white/10 disabled:opacity-50"
            >
              <Package className="size-4" />
            </button>
          </Tooltip>
        )}



        {/* Dropdown Menu (Extra / Danger) */}
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <button
              aria-label={t("buttons.more_options")}
              className="hover-shimmer flex size-9 items-center justify-center rounded-md bg-white/5 text-foreground transition-colors hover:bg-white/10"
            >
              <MoreHorizontal className="size-4" />
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content
              align="end"
              className="z-50 min-w-40 rounded-md border border-white/10 bg-black/40 p-1 shadow-[0_16px_40px_rgba(0,0,0,0.6)] backdrop-blur-xl animate-in fade-in zoom-in-95"
            >
              {showInstalledActions && installedHandle && (
                <DropdownMenu.Item
                  onClick={() =>
                    kind === "installed" && onOpenFolder
                      ? onOpenFolder(installedHandle)
                      : void overlayOpenFolder()
                  }
                  className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-white/10"
                >
                  <FolderOpen className="size-4" />
                  {t("tooltips.open_folder")}
                </DropdownMenu.Item>
              )}

              <DropdownMenu.Item
                onClick={() => void openWorkshopPage()}
                className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-white/10"
              >
                <ExternalLink className="size-4" />
                {t("buttons.open_workshop")}
              </DropdownMenu.Item>

              {copyIdHandle && (
                <DropdownMenu.Item
                  onClick={async () => {
                    if (typeof navigator !== "undefined" && navigator.clipboard) {
                      await navigator.clipboard.writeText(copyIdHandle.pubfileid);
                      pushToast(t("messages.copied") || "Copied", "success");
                    }
                  }}
                  className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-white/10"
                >
                  <Copy className="size-4" />
                  Copy ID
                </DropdownMenu.Item>
              )}
              {showInstalledActions && installedHandle && (
                <>
                  <DropdownMenu.Separator className="my-1 h-px bg-white/10" />
                  <DropdownMenu.Item
                    onClick={() =>
                      kind === "installed" && onDelete
                        ? onDelete(installedHandle)
                        : void overlayDelete()
                    }
                    className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-sm text-danger outline-none hover:bg-danger/20 focus:bg-danger/20"
                  >
                    <Trash2 className="size-4" />
                    {t("tooltips.delete_wallpaper")}
                  </DropdownMenu.Item>
                </>
              )}
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
      </div>
      {ConfirmDialog}
    </>
  );
}
