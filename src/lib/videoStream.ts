/**
 * Client for the "watch while downloading" feature.
 *
 * Video wallpapers are streamed to an external player (mpv/VLC) over a local
 * HTTP range URL served by the Rust side, so playback can start before the
 * download finishes and seeking works without downloading everything.
 */

import { invoke } from "@/lib/tauri";

export interface PlayerInfo {
  name: string;
  path: string;
}

export interface StreamStarted {
  item_id: number;
  url: string;
  filename: string;
  total_size: number;
  player: string;
}

/** Which external player the app found, or null when none is installed. */
export async function getStreamPlayer(): Promise<PlayerInfo | null> {
  try {
    return await invoke<PlayerInfo | null>("video_stream_player");
  } catch {
    return null;
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

/** Stop the stream for one item. Returns whether one was running. */
export async function stopVideoStream(itemId: string | number): Promise<boolean> {
  const id = typeof itemId === "number" ? itemId : Number(itemId);
  return await invoke<boolean>("video_stream_stop", { itemId: id });
}

/** Stop every stream (call on shutdown). */
export async function stopAllVideoStreams(): Promise<number> {
  return await invoke<number>("video_stream_stop_all");
}