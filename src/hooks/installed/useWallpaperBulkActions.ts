import type { Dispatch, SetStateAction } from "react";
import type { TranslationKey } from "@/i18n/types";
import { useTranslation } from "@/i18n/hooks";
import { open as openPath } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { inTauri, tryInvoke, tryInvokeAction, invoke } from "@/lib/tauri";
import { maybeMinimize } from "@/lib/window";
import { pushToast } from "@/stores/toasts";
import { EVENTS } from "@/lib/events";
import { InstalledWallpaper } from "@/types/workshop";
import { useConfirm } from "@/hooks/useConfirm";
import { useMetadataInitStore } from "@/stores/metadata-init";

interface UseWallpaperBulkActionsArgs {
  t: ReturnType<typeof useTranslation>["t"];
  confirm: ReturnType<typeof useConfirm>["confirm"];
  items: InstalledWallpaper[];
  setItems: Dispatch<SetStateAction<InstalledWallpaper[]>>;
  refresh: () => Promise<void>;
  refreshInstalledGlobal: () => void;
  selectedIds: Set<string>;
  setSelectedIds: Dispatch<SetStateAction<Set<string>>>;
  setSelectionMode: Dispatch<SetStateAction<boolean>>;
  setSelected: Dispatch<SetStateAction<InstalledWallpaper | null>>;
}

/**
 * Every mutating command triggered from the Installed view — single-item
 * (apply / delete / extract / open folder / copy id / init metadata) and
 * bulk (delete / extract / copy id) — pulled out of `useWallpaperActions`.
 */
export function useWallpaperBulkActions({
  t,
  confirm,
  items,
  setItems,
  refresh,
  refreshInstalledGlobal,
  selectedIds,
  setSelectedIds,
  setSelectionMode,
  setSelected,
}: UseWallpaperBulkActionsArgs) {
  const handleApply = async (item: InstalledWallpaper) => {
    if (!inTauri) {
      pushToast(`Apply ${item.pubfileid}`, "info");
      return;
    }
    const res = await tryInvokeAction("we_apply", {
      projectPath: item.project_json_path,
      monitor: null,
      force: false,
    });
    pushToast(
      res.ok
        ? t("messages.wallpaper_applied")
        : `${t("messages.error") || "Error"}: ${res.error}`,
      res.ok ? "success" : "error",
    );
    if (res.ok) {
      void maybeMinimize();
    }
  };

  const handleDelete = async (item: InstalledWallpaper) => {
    // Workshop folders belong to the Steam client: deleting one just makes
    // Steam re-download it, and can leave its subscription state confused.
    if (item.source === "workshop") {
      pushToast(
        t("messages.workshop_item_not_deletable") ||
          "Steam Workshop wallpapers are managed by Steam — unsubscribe there to remove it.",
        "warning",
      );
      return;
    }
    if (inTauri) {
      const active = await tryInvoke<string[]>(
        "we_active_pubfileids",
        undefined,
        [],
      );
      if ((active ?? []).includes(item.pubfileid)) {
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

    const previousItems = items;
    setItems((prev) => prev.filter((i) => i.pubfileid !== item.pubfileid));
    setSelected(null);

    if (!inTauri) {
      pushToast(t("messages.wallpaper_deleted"), "success");
      return;
    }

    const res = await tryInvokeAction("we_delete_wallpaper", {
      pubfileid: item.pubfileid,
    });

    if (res.ok) {
      pushToast(t("messages.wallpaper_deleted"), "success");
      void refresh();
      void refreshInstalledGlobal();
    } else {
      setItems(previousItems);
      pushToast(describeDeleteError(res.error, t), "error");
    }
  };

  const handleBulkDelete = async () => {
    if (selectedIds.size === 0) return;

    const workshopIds = new Set(
      items.filter((i) => selectedIds.has(i.pubfileid) && i.source === "workshop").map((i) => i.pubfileid),
    );
    if (workshopIds.size > 0) {
      pushToast(
        t("messages.bulk_delete_has_workshop", { count: workshopIds.size }) ||
          `${workshopIds.size} of the selected wallpapers are Steam Workshop items and cannot be deleted here.`
        ,
        "error",
      );
      return;
    }

    if (inTauri) {
      const active = await tryInvoke<string[]>(
        "we_active_pubfileids",
        undefined,
        [],
      );
      const activeInSelection = Array.from(selectedIds).filter((id) =>
        (active ?? []).includes(id),
      );
      if (activeInSelection.length > 0) {
        pushToast(
          t("messages.bulk_delete_with_active", {
            count: activeInSelection.length,
          }),
          "error",
        );
        return;
      }
    }

    const confirmed = await confirm({
      title: t("buttons.delete"),
      message: t("messages.confirm_bulk_delete", { count: selectedIds.size }),
      confirmLabel: t("buttons.delete") || "Delete",
      cancelLabel: t("buttons.cancel") || "Cancel",
      variant: "danger",
    });
    if (!confirmed) return;

    const previousItems = items;
    const toDelete = selectedIds;
    setItems((prev) => prev.filter((i) => !toDelete.has(i.pubfileid)));
    setSelectedIds(new Set());
    setSelectionMode(false);
    setSelected(null);

    if (!inTauri) {
      pushToast(
        t("messages.bulk_delete_success", { count: toDelete.size }),
        "success",
      );
      return;
    }

    // One bulk call, not N concurrent single deletes.
    //
    // `Promise.all` over `we_delete_wallpaper` used to fire a dozen
    // simultaneous commands. Each re-read config.json, re-walked three
    // directories and rewrote the whole metadata.json, which locked up the
    // window. The backend now takes the whole list in one shot.
    const failure = await tryInvokeAction<Array<[string, string]>>(
      "we_delete_wallpapers_bulk",
      { pubfileids: Array.from(toDelete) },
    );
    const failed = failure.ok ? (failure.value ?? []) : [];
    const successCount = toDelete.size - failed.length;

    if (successCount > 0) {
      pushToast(
        t("messages.bulk_delete_success", { count: successCount }),
        "success",
      );
      void refresh();
      void refreshInstalledGlobal();
    } else {
      setItems(previousItems);
      const errorMsg = failure.ok
        ? failed[0]?.[1] ?? "Unknown error"
        : failure.error;
      pushToast(describeDeleteError(errorMsg, t), "error");
    }
  };

  const handleBulkExtract = async () => {
    if (selectedIds.size === 0) return;

    const selectedItems = items.filter((item) =>
      selectedIds.has(item.pubfileid),
    );
    const withPkg = selectedItems.filter((item) => item.has_pkg);

    if (withPkg.length === 0) {
      pushToast(t("messages.no_pkg_file"), "warning");
      return;
    }

    if (!inTauri) {
      pushToast(
        t("messages.bulk_extract_success", { count: withPkg.length }),
        "success",
      );
      return;
    }

    const folder = await openPath({ directory: true });
    if (!folder || Array.isArray(folder)) return;

    // Throttled, not unbounded.
    //
    // Each extract spawns a RepKG (.NET) process that reads a multi-hundred-
    // megabyte .pkg and writes it back out. Firing one per selected wallpaper
    // at once saturated the disk and starved the UI thread — selecting 20
    // wallpapers could hang the window for minutes. Two at a time keeps the
    // machine responsive while still finishing well ahead of serial.
    const CONCURRENCY = 2;
    const results: Awaited<ReturnType<typeof tryInvokeAction>>[] = [];
    for (let i = 0; i < withPkg.length; i += CONCURRENCY) {
      const chunk = withPkg.slice(i, i + CONCURRENCY);
      const chunkResults = await Promise.all(
        chunk.map((item) =>
          tryInvokeAction("extract_start", {
            pubfileid: item.pubfileid,
            outputDir: folder,
          }),
        ),
      );
      results.push(...chunkResults);
    }
    const successCount = results.filter((r) => r.ok).length;

    if (successCount > 0) {
      pushToast(
        t("messages.bulk_extract_success", { count: successCount }),
        "success",
      );
    } else {
      const errorMsg = results.find((r) => !r.ok)?.error || "Unknown error";
      pushToast(
        `${t("messages.bulk_extract_success", { count: 0 })}: ${errorMsg}`,
        "error",
      );
    }
    setSelectedIds(new Set());
    setSelectionMode(false);
  };

  const handleExtract = async (item: InstalledWallpaper) => {
    if (!item.has_pkg) {
      pushToast(t("messages.no_pkg_file"), "warning");
      return;
    }
    if (!inTauri) {
      pushToast(t("messages.extraction_started"), "success");
      return;
    }
    const folder = await openPath({ directory: true });
    if (!folder || Array.isArray(folder)) return;
    const res = await tryInvokeAction("extract_start", {
      pubfileid: item.pubfileid,
      outputDir: folder,
    });
    pushToast(
      res.ok
        ? t("messages.extraction_started")
        : `${t("messages.error") || "Error"}: ${res.error}`,
      res.ok ? "success" : "error",
    );
  };

  const handleOpenFolder = async (item: InstalledWallpaper) => {
    if (!inTauri) return;
    await tryInvoke("open_path", { path: item.folder });
  };

  const handleCopyId = async (item: InstalledWallpaper) => {
    await navigator.clipboard.writeText(item.pubfileid);
    pushToast(t("messages.id_copied"), "success");
  };

  const handleBulkCopyId = async () => {
    if (selectedIds.size === 0) return;
    const idsString = Array.from(selectedIds).join(" ");
    await navigator.clipboard.writeText(idsString);
    pushToast(t("messages.id_copied"), "success");
  };

  const handleInitMetadata = async () => {
    if (!inTauri) return;
    const setStatus = useMetadataInitStore.getState().setStatus;

    setStatus({
      phase: "initializing",
      message:
        t("metadata_init.fetching") ||
        "Fetching metadata for installed wallpapers...",
      progress: 0,
      total: items.length,
    });

    let lastUpdate = 0;
    const unlisten = await listen<{ current: number; total: number }>(
      EVENTS.METADATA_INIT_PROGRESS,
      (event) => {
        const now = Date.now();
        if (now - lastUpdate > 33) {
          setStatus({
            phase: "initializing",
            message:
              t("metadata_init.fetching") ||
              "Fetching metadata for installed wallpapers...",
            progress: event.payload.current,
            total: event.payload.total,
          });
          lastUpdate = now;
        }
      },
    );

    try {
      const count = await invoke<number>("app_init_metadata");

      setStatus({
        phase: "complete",
        message:
          t("labels.metadata_initialized", { count: count ?? 0 }) ||
          `Initialized ${count ?? 0} wallpapers`,
        progress: count ?? 0,
        total: items.length,
      });

      void refresh();
    } catch (error) {
      const errorMessage =
        error instanceof Error ? error.message : String(error);
      const isRateLimit =
        errorMessage.includes("429") ||
        errorMessage.toLowerCase().includes("rate limit");

      setStatus({
        phase: "error",
        message: isRateLimit
          ? t("metadata_init.rate_limit_error") ||
            "Rate limit exceeded. Please try again in a few minutes."
          : t("metadata_init.error_message") || "Failed to initialize metadata",
        progress: null,
        total: null,
      });
    } finally {
      unlisten();
    }
  };

  return {
    handleApply,
    handleDelete,
    handleBulkDelete,
    handleBulkExtract,
    handleBulkCopyId,
    handleExtract,
    handleOpenFolder,
    handleCopyId,
    handleInitMetadata,
  };
}

/**
 * Turn a backend delete error into something a user can act on.
 *
 * The command returns machine-readable prefixes (ACTIVE:/NOTFOUND:/
 * WORKSHOP:) precisely so the UI can explain what happened instead of
 * showing a raw string — a silent no-op is what made deletes look like they
 * worked and then reappear on refresh.
 */
function describeDeleteError(
  raw: string,
  t: (key: TranslationKey) => string,
): string {
  if (raw.startsWith("WORKSHOP:")) {
    return t("messages.delete_workshop_blocked");
  }
  if (raw.startsWith("NOTFOUND:")) {
    return t("messages.delete_not_found");
  }
  if (raw.startsWith("ACTIVE:")) {
    return t("messages.cannot_delete_active_single");
  }
  return `${t("messages.error")}: ${raw}`;
}
