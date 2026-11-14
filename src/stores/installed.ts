import { create } from "zustand";

import { inTauri, tryInvoke } from "@/lib/tauri";
import type { InstalledMetadata, InstalledWallpaper } from "@/types/workshop";

/**
 * Global cache of currently-installed wallpapers, keyed by pubfileid.
 *
 * Workshop / Collection / Author cards consult this cache to:
 *   1. Render an "Installed" indicator on cards (a small ✓ badge).
 *   2. Switch the card's primary action from "Install" → "Apply" so the
 *      DetailsPanel for an item that's *already on disk* matches the
 *      Installed-mode panel even outside the Installed tab.
 */
interface InstalledState {
  byId: Record<string, InstalledWallpaper>;
  /// Author / tags / rating per pubfileid, cached alongside `byId`.
  ///
  /// The Installed grid renders the author line and derives its tag and author
  /// filters from this map. It used to live only in `useWallpaperActions`, so
  /// the cache-hit path could seed `items` without it and the first frame
  /// showed "—" for every author until `metadata_get_all` came back. Keeping
  /// both in one store means the warm-up fills them together and the grid's
  /// first paint is already complete.
  metaMap: Record<string, InstalledMetadata>;
  ready: boolean;
  updateCounter: number;
  refresh: () => Promise<void>;
  /// Replace the cache. `metaMap` is optional so callers that only know the
  /// file list (optimistic updates) do not wipe the metadata that is already
  /// cached.
  setAll: (
    items: InstalledWallpaper[],
    metaMap?: Record<string, InstalledMetadata>,
  ) => void;
  isInstalled: (pubfileid: string) => boolean;
  get: (pubfileid: string) => InstalledWallpaper | undefined;
  addOptimistic: (pubfileid: string, wallpaper: InstalledWallpaper) => void;
  removeOptimistic: (pubfileid: string) => void;
}

export const useInstalledStore = create<InstalledState>((set, get) => ({
  byId: {},
  metaMap: {},
  ready: false,
  updateCounter: 0,
  setAll: (items, metaMap) => {
    const byId: Record<string, InstalledWallpaper> = {};
    for (const w of items) byId[w.pubfileid] = w;
    set((state) => ({
      byId,
      ...(metaMap ? { metaMap } : {}),
      ready: true,
      updateCounter: state.updateCounter + 1,
    }));
  },
  refresh: async () => {
    if (!inTauri) {
      set({ ready: true });
      return;
    }
    // `force` bypasses the backend's scan cache. This method is called after
    // a download, a deletion or a manual refresh — the only moments the list
    // can actually change — so paying for a real scan here is correct. View
    // switches call `we_list_installed` without it and get the cached result.
    // Fetch the list and its metadata together: both are needed before the
    // grid can render correctly, and issuing them in sequence would double the
    // wait on a cold cache.
    const [items, meta] = await Promise.all([
      tryInvoke<InstalledWallpaper[]>("we_list_installed", { force: true }, []),
      tryInvoke<Record<string, InstalledMetadata>>(
        "metadata_get_all",
        undefined,
        {},
      ),
    ]);
    get().setAll(items ?? [], meta ?? {});
  },
  isInstalled: (pubfileid) => Boolean(get().byId[pubfileid]),
  get: (pubfileid) => get().byId[pubfileid],
  addOptimistic: (pubfileid, wallpaper) => {
    set((state) => ({
      byId: { ...state.byId, [pubfileid]: wallpaper },
    }));
  },
  removeOptimistic: (pubfileid) => {
    set((state) => {
      const { [pubfileid]: _, ...rest } = state.byId;
      return { byId: rest };
    });
  },
}));
