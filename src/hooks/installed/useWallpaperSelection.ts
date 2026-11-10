import { useState } from "react";
import { InstalledWallpaper } from "@/types/workshop";

interface UseWallpaperSelectionArgs {
  /** Currently visible (post-filter) wallpapers — `selectAll` targets these. */
  filtered: InstalledWallpaper[];
}

/**
 * Multi-select state for the Installed view: the single "details" selection
 * plus the bulk-checkbox selection. Extracted from `useWallpaperActions`.
 */
export function useWallpaperSelection({ filtered }: UseWallpaperSelectionArgs) {
  const [selected, setSelected] = useState<InstalledWallpaper | null>(null);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [selectionMode, setSelectionMode] = useState(false);

  const toggleSelection = (pubfileid: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(pubfileid)) {
        next.delete(pubfileid);
      } else {
        next.add(pubfileid);
      }
      return next;
    });
  };

  const selectAll = () => {
    setSelectedIds(new Set(filtered.map((item) => item.pubfileid)));
  };

  const clearSelection = () => {
    setSelectedIds(new Set());
  };

  return {
    selected,
    setSelected,
    selectedIds,
    setSelectedIds,
    selectionMode,
    setSelectionMode,
    toggleSelection,
    selectAll,
    clearSelection,
  };
}
