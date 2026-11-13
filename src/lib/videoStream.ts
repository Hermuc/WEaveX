/**
 * Client for the "watch while downloading" feature.
 *
 * Video wallpapers are streamed to an external player (mpv/VLC) over a local
 * HTTP range URL served by the Rust side, so playback can start before the
 * download finishes and seeking works without downloading everything.
 */

import { invoke } from "@/lib/tauri";


export interface StreamStarted {
  item_id: number;
  url: string;
  filename: string;
  total_size: number;
  player: string;
}

/**
 * Kick off background resolution of a workshop video so a later "watch" click
 * is instant. Fire-and-forget; errors are logged server-side.
 */
export async function prewarmVideoStream(itemId: string | number): Promise<void> {
  const id = typeof itemId === "number" ? itemId : Number(itemId);
  try {
    await invoke("video_stream_prewarm", { itemId: id });
  } catch {
    // best-effort; the start path will resolve inline if prewarm failed
  }
}


/**
 * Resolve the item, start a range server and launch the player.
 * Throws a short code string ("NO_PLAYER" / "NO_ACCOUNT") or a message.
 */
export async function startVideoStream(itemId: string | number): Promise<StreamStarted> {
  const id = typeof itemId === "number" ? itemId : Number(itemId);
  return await invoke<StreamStarted>("video_stream_start", { itemId: id });
}

