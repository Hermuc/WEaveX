import { create } from "zustand";

import type { UpdateInfo } from "@/types/updater";

interface UpdaterState {
  info: UpdateInfo | null;
  bannerVisible: boolean;
  dialogOpen: boolean;
  show: (info: UpdateInfo) => void;
  openDialog: () => void;
  closeDialog: () => void;
  dismiss: () => void;
}

/**
 * Cache of the most recent update-check result. Used so that auto-check on
 * bootstrap can surface the update dialog and banner automatically.
 */
export const useUpdaterStore = create<UpdaterState>((set) => ({
  info: null,
  bannerVisible: false,
  dialogOpen: false,
  show: (info) =>
    set((state) => ({
      info,
      bannerVisible: Boolean(info.update_available && !info.error),
      dialogOpen: Boolean(info.update_available && !info.error) || state.dialogOpen,
    })),
  openDialog: () => set({ dialogOpen: true }),
  closeDialog: () => set({ dialogOpen: false }),
  dismiss: () => set({ bannerVisible: false, dialogOpen: false }),
}));
