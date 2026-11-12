import { useCallback, useState } from "react";
import { useTranslation } from "@/i18n/hooks";
import { pushToast } from "@/stores/toasts";
import { startVideoStream } from "@/lib/videoStream";
import type { WorkshopItem } from "@/types/workshop";

export interface WatchState {
  /** Item currently being resolved/launched, or null when idle. */
  pendingId: string | null;
  /** Last error message for inline display, cleared on the next attempt. */
  error: string | null;
}

/**
 * Shared "watch while downloading" handler.
 *
 * Video wallpapers are handed to an external player over a local HTTP range URL,
 * so playback starts immediately and seeking does not wait for the download.
 *
 * Feedback is returned as state rather than toasts: the details panel sets
 * `detailsOpen`, and the toast store deliberately suppresses toasts while a
 * panel is open — so a toast-based spinner would be invisible exactly where
 * this button lives. The caller renders a spinner from `pendingId` instead.
 */
export function useWatchWhileDownloading(): {
  watch: (item: WorkshopItem) => Promise<void>;
  state: WatchState;
} {
  const { t } = useTranslation();
  const [state, setState] = useState<WatchState>({ pendingId: null, error: null });

  const watch = useCallback(
    async (item: WorkshopItem) => {
      // Collections have no single video to stream.
      if (item.is_collection) return;

      setState({ pendingId: item.pubfileid, error: null });
      try {
        await startVideoStream(item.pubfileid);
        setState({ pendingId: null, error: null });
        // Success toast also works when the panel closes (detailsOpen false by
        // then), so the user still gets confirmation.
        pushToast(
          t("messages.streaming_started", { player: "mpv" }),
          "success",
        );
      } catch (err) {
        const raw = err instanceof Error ? err.message : String(err);
        let msg: string;
        if (raw.includes("NO_PLAYER")) {
          msg = t("messages.streaming_no_player");
        } else if (raw.includes("NO_ACCOUNT")) {
          msg = t("messages.streaming_no_account");
        } else {
          msg = t("messages.streaming_failed", { error: raw });
        }
        setState({ pendingId: null, error: msg });
      }
    },
    [t],
  );

  return { watch, state };
}