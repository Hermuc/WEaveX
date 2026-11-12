import type { InstalledWallpaper, WorkshopItem } from "./workshop";

/**
 * Props contracts shared by the details panel (thin wrapper) and the details
 * sidebar (the real implementation).
 *
 * These were duplicated verbatim in
 * `components/common/DetailsPanel.tsx` and
 * `components/common/details/DetailsSidebar.tsx`; keeping one copy here means
 * the wrapper and the implementation can never drift apart.
 */

export interface CommonProps {
  onClose: () => void;
}

export interface WorkshopProps extends CommonProps {
  kind: "workshop";
  item: WorkshopItem | null;
  onDownload: (item: WorkshopItem) => void;
  /// Stream the video to an external player while downloading (video wallpapers).
  onWatch?: (item: WorkshopItem) => void;
  /// Progress/error state for the watch button (toasts are suppressed while a
  /// details panel is open, so the button must render feedback itself).
  watchState?: { pendingId: string | null; error: string | null };
}

export interface InstalledProps extends CommonProps {
  kind: "installed";
  item: InstalledWallpaper | null;
  onApply: (item: InstalledWallpaper) => void;
  onExtract: (item: InstalledWallpaper) => void;
  onDelete: (item: InstalledWallpaper) => void;
  onOpenFolder: (item: InstalledWallpaper) => void;
  onCopyId: (item: InstalledWallpaper) => void;
}
