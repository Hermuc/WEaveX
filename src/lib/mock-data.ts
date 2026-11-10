import { InstalledWallpaper } from "@/types/workshop";

/**
 * Synthetic wallpaper list used only when the UI runs outside Tauri
 * (plain browser / unit tests without the Rust backend). Kept in a
 * dependency-free module so production bundles never pull in test
 * tooling (vitest).
 */
export function makeMockInstalled(): InstalledWallpaper[] {
  return Array.from({ length: 6 }, (_, i) => ({
    pubfileid: `${2000000 + i}`,
    folder: `/mock/projects/myprojects/${2000000 + i}`,
    project_json_path: `/mock/projects/myprojects/${2000000 + i}/project.json`,
    has_pkg: i % 2 === 0,
    title: `Installed Wallpaper #${i + 1}`,
    preview: "",
    description:
      "Mock description shown when running outside Tauri. Includes more information about the wallpaper.",
    file_type: i % 3 === 0 ? "scene" : i % 3 === 1 ? "video" : "web",
    tags: i % 2 === 0 ? ["Anime", "3D"] : ["Nature", "Abstract"],
    size_bytes: (i + 1) * 30 * 1024 * 1024,
    installed_ts: Date.now() / 1000 - i * 86400,
  }));
}
