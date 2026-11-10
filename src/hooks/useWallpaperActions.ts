import { useState, useEffect } from "react";
import { useTranslation } from "@/i18n/hooks";
import { useAppStore } from "@/stores/app";
import { useInstalledStore } from "@/stores/installed";
import { inTauri, tryInvoke } from "@/lib/tauri";
import { useRefreshStore } from "@/stores/refresh";
import { InstalledWallpaper, InstalledMetadata } from "@/types/workshop";

export type { InstalledMetadata };

import { useConfirm } from "@/hooks/useConfirm";
import { makeMockInstalled } from "@/lib/mock-data";
import { useInstalledFilter } from "@/hooks/installed/useInstalledFilter";
import { useWallpaperSelection } from "@/hooks/installed/useWallpaperSelection";
import { useWallpaperBulkActions } from "@/hooks/installed/useWallpaperBulkActions";

/**
 * Orchestrator for the Installed view. It owns only the data-loading state
 * (`items` / `loading` / `metaMap`) and the refresh effects; every other
 * concern lives in a focused child hook:
 *   - filtering / sorting   → useInstalledFilter
 *   - selection state       → useWallpaperSelection
 *   - mutating commands     → useWallpaperBulkActions
 * The returned object shape is unchanged so existing consumers keep working.
 */
export function useWallpaperActions() {
  const { t } = useTranslation();
  const { confirm, ConfirmDialog } = useConfirm();
  const weDirectory = useAppStore((s) => s.weDirectory);
  const refreshInstalledGlobal = useInstalledStore((s) => s.refresh);
  const installedUpdateCounter = useInstalledStore((s) => s.updateCounter);

  const [items, setItems] = useState<InstalledWallpaper[]>([]);
  const [loading, setLoading] = useState(true);
  const [metaMap, setMetaMap] = useState<Record<string, InstalledMetadata>>({});

  const refresh = async () => {
    if (!inTauri) {
      setItems(makeMockInstalled());
      setLoading(false);
      return;
    }
    setLoading(true);
    const list = await tryInvoke<InstalledWallpaper[]>(
      "we_list_installed",
      undefined,
      [],
    );
    setItems(list ?? []);
    const meta = await tryInvoke<Record<string, InstalledMetadata>>(
      "metadata_get_all",
      undefined,
      {},
    );
    setMetaMap(meta ?? {});
    setLoading(false);
  };

  useEffect(() => {
    const t = setTimeout(() => {
      void refresh();
    }, 0);
    return () => clearTimeout(t);
  }, [weDirectory]);

  const refreshCounter = useRefreshStore((s) => s.counter);
  useEffect(() => {
    if (refreshCounter === 0) return;
    const t = setTimeout(() => {
      void refresh();
    }, 0);
    return () => clearTimeout(t);
  }, [refreshCounter, weDirectory]);

  useEffect(() => {
    if (installedUpdateCounter > 0) {
      const t = setTimeout(() => {
        void refresh();
      }, 0);
      return () => clearTimeout(t);
    }
  }, [installedUpdateCounter]);

  const filter = useInstalledFilter({ items, metaMap });
  const selection = useWallpaperSelection({ filtered: filter.filtered });
  const actions = useWallpaperBulkActions({
    t,
    confirm,
    items,
    setItems,
    refresh,
    refreshInstalledGlobal,
    selectedIds: selection.selectedIds,
    setSelectedIds: selection.setSelectedIds,
    setSelectionMode: selection.setSelectionMode,
    setSelected: selection.setSelected,
  });

  useEffect(() => {
    if (!inTauri || selection.selected !== null) return;
    void (async () => {
      const meta = await tryInvoke<Record<string, InstalledMetadata>>(
        "metadata_get_all",
        undefined,
        {},
      );
      if (meta) setMetaMap(meta);
    })();
  }, [selection.selected]);

  return {
    items,
    setItems,
    loading,
    metaMap,
    refresh,
    ...filter,
    ...selection,
    ...actions,
    ConfirmDialog,
  };
}
