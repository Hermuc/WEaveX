import { useState, useEffect, useCallback, useRef } from "react";
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

  // Tracks whether a first load has finished, so the skeleton is only shown
  // before any data exists rather than on every tab switch. A ref (not state)
  // keeps refresh stable for the dependency arrays below.
  const hasLoadedRef = useRef(false);

  const refresh = useCallback(async (opts?: { force?: boolean }) => {
    if (!inTauri) {
      setItems(makeMockInstalled());
      setLoading(false);
      hasLoadedRef.current = true;
      return;
    }

    // Reuse the cache the bootstrap step filled at startup instead of scanning
    // the disk again on every visit.
    const cachedStore = useInstalledStore.getState();
    const cachedItems = Object.values(cachedStore.byId);
    if (!opts?.force && cachedStore.ready && cachedItems.length > 0) {
      setItems(cachedItems);
      setLoading(false);
      hasLoadedRef.current = true;
    } else if (!hasLoadedRef.current) {
      setLoading(true);
    }

    const list = await tryInvoke<InstalledWallpaper[]>(
      "we_list_installed",
      undefined,
      [],
    );
    const next = list ?? [];
    setItems(next);
    // NOTE: deliberately not writing back to the global store here. setAll()
    // bumps updateCounter, which this hook also watches, so writing back
    // would re-enter refresh() forever. The global store is owned by bootstrap
    // and by the download/delete paths, which refresh it explicitly.

    const meta = await tryInvoke<Record<string, InstalledMetadata>>(
      "metadata_get_all",
      undefined,
      {},
    );
    setMetaMap(meta ?? {});
    setLoading(false);
    hasLoadedRef.current = true;
  }, []);

  useEffect(() => {
    // weDirectory starts empty and is filled during bootstrap; waiting for the
    // real path avoids a scan that would immediately be repeated.
    if (!weDirectory) return;
    const t = setTimeout(() => {
      void refresh();
    }, 0);
    return () => clearTimeout(t);
  }, [weDirectory, refresh]);

  const refreshCounter = useRefreshStore((s) => s.counter);
  useEffect(() => {
    if (refreshCounter === 0) return;
    const t = setTimeout(() => {
      void refresh();
    }, 0);
    return () => clearTimeout(t);
  }, [refreshCounter, weDirectory, refresh]);

  useEffect(() => {
    if (installedUpdateCounter > 0) {
      const t = setTimeout(() => {
        void refresh();
      }, 0);
      return () => clearTimeout(t);
    }
  }, [installedUpdateCounter, refresh]);

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
