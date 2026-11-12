import { useCallback } from "react";
import { useTranslation } from "@/i18n/hooks";
import { pushToast } from "@/stores/toasts";
import { startVideoStream } from "@/lib/videoStream";
import type { WorkshopItem } from "@/types/workshop";

/**
 * Shared "watch while downloading" handler.
 *
 * Video wallpapers are handed to an external player over a local HTTP range URL,
 * so playback starts immediately and seeking does not wait for the download.
 * The Rust side owns the stream lifecycle; this hook only reports outcomes.
 *
 * Kept as a hook so the three workshop views (browse / author / collections)
 * share one implementation instead of three copies of the same toast logic.
 */
export function useWatchWhileDownloading(): (item: WorkshopItem) => Promise<void> {
  const { t } = useTranslation();

  return useCallback(
    async (item: WorkshopItem) => {
      // Collections have no single video to stream.
      if (item.is_collection) return;

      pushToast(t("messages.streaming_starting"), "info");

      try {
        const res = await startVideoStream(item.pubfileid);
        pushToast(
          t("messages.streaming_started", { player: res.player }),
          "success",
        );
      } catch (err) {
        const raw = err instanceof Error ? err.message : String(err);
        if (raw.includes("NO_PLAYER")) {
          pushToast(t("messages.streaming_no_player"), "error");
        } else if (raw.includes("NO_ACCOUNT")) {
          pushToast(t("messages.streaming_no_account"), "error");
        } else {
          pushToast(t("messages.streaming_failed", { error: raw }), "error");
        }
      }
    },
    [t],
  );
}