import { describe, it, expect, beforeEach } from "vitest";

import { useInstalledStore } from "./installed";
import type { InstalledWallpaper } from "@/types/workshop";

function makeWallpaper(pubfileid: string): InstalledWallpaper {
  return {
    pubfileid,
    folder: `/tmp/${pubfileid}`,
    project_json_path: `/tmp/${pubfileid}/project.json`,
    has_pkg: false,
    title: `Wallpaper ${pubfileid}`,
    preview: "",
    description: "",
    file_type: "video",
    tags: [],
    content_rating: "Everyone",
    size_bytes: 1024,
    installed_ts: 0,
    source: "workshop",
  };
}

describe("useInstalledStore", () => {
  beforeEach(() => {
    // Reset to the initial shape between tests; the store is a module-level
    // singleton, so state would otherwise leak across cases.
    useInstalledStore.setState({
      byId: {},
      metaMap: {},
      ready: false,
      updateCounter: 0,
    });
  });

  it("stores the file list and metadata together", () => {
    // The Installed grid needs both to render a complete card: `byId` drives
    // the list and `metaMap` supplies the author line. Seeding only the list
    // is what produced the half-rendered first frame.
    useInstalledStore.getState().setAll(
      [makeWallpaper("111"), makeWallpaper("222")],
      { "111": { author: "Alice" } },
    );

    const state = useInstalledStore.getState();
    expect(Object.keys(state.byId)).toEqual(["111", "222"]);
    expect(state.metaMap["111"]?.author).toBe("Alice");
    expect(state.ready).toBe(true);
  });

  it("keeps existing metadata when a caller only supplies the list", () => {
    // Optimistic update paths (apply/delete) know the list but not the
    // metadata; they must not wipe what is already cached.
    useInstalledStore.getState().setAll([makeWallpaper("111")], {
      "111": { author: "Alice" },
    });
    useInstalledStore.getState().setAll([makeWallpaper("111"), makeWallpaper("333")]);

    const state = useInstalledStore.getState();
    expect(state.metaMap["111"]?.author).toBe("Alice");
    expect(Object.keys(state.byId)).toEqual(["111", "333"]);
  });

  it("bumps updateCounter so the view re-reads the cache", () => {
    const before = useInstalledStore.getState().updateCounter;
    useInstalledStore.getState().setAll([makeWallpaper("111")], {});
    expect(useInstalledStore.getState().updateCounter).toBe(before + 1);
  });

  it("reports installed ids from the cached list", () => {
    useInstalledStore.getState().setAll([makeWallpaper("111")], {});
    expect(useInstalledStore.getState().isInstalled("111")).toBe(true);
    expect(useInstalledStore.getState().isInstalled("999")).toBe(false);
  });
});
