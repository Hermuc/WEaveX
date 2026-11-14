import { useEffect, useMemo, useRef, useState } from "react";

import { useRefreshStore } from "@/stores/refresh";
import { useSteamSessionStore } from "@/stores/steam-session";
import { inTauri, tryInvoke, tryInvokeOk } from "@/lib/tauri";
import type { WorkshopFilters } from "@/stores/filters";
import type { WorkshopPage } from "@/types/workshop";

// Browser-only fallback data, used when the app runs outside Tauri (e.g.
// `vite dev` in a plain browser). Kept in one place so the workshop and
// collections views cannot drift apart again — they previously carried two
// near-identical copies that had already diverged (30 vs 18 fake rows).
export function makeMockPage(kind: "workshop" | "collection", page: number): WorkshopPage {
  const perPage = kind === "collection" ? 30 : 18;
  const items = Array.from({ length: perPage }, (_, i) => ({
    pubfileid: `${(kind === "collection" ? 500000 : 100000) + (page - 1) * perPage + i}`,
    title:
      kind === "collection"
        ? `Mock Collection #${(page - 1) * perPage + i + 1}`
        : `Sample Wallpaper #${(page - 1) * perPage + i + 1}`,
    preview_url: "",
    author: kind === "collection" ? "Author" : "Mock Author",
    author_url: "",
    description: "",
    file_size: "",
    posted_date: "",
    updated_date: "",
    tags: [],
    rating_star_file: "",
    num_ratings: "",
    is_collection: kind === "collection",
  }));
  return {
    items,
    total_items: kind === "collection" ? 120 : 500,
    total_pages: kind === "collection" ? 5 : 10,
    current_page: page,
  };
}

interface UseWorkshopPageOptions {
  /** The Tauri command that fetches a page. */
  cmd: string;
  filters: WorkshopFilters;
  /** When false the hook stays idle (e.g. a collection is opened, so the list
   *  view underneath must not fetch). */
  enabled?: boolean;
  /** Extra value mixed into the cache key so two collections do not share
   *  cached pages. */
  cacheScope?: string | null;
  /** Whether to prefetch the next page in the background. */
  preloadNext?: boolean;
  /** Browser-only fallback; required when running outside Tauri. */
  mockKind?: "workshop" | "collection";
}

interface UseWorkshopPageResult {
  page: WorkshopPage | null;
  loading: boolean;
  cache: React.MutableRefObject<Map<string, WorkshopPage>>;
}

/**
 * Fetch a paginated workshop page, with a preload cache and the
 * steam-phase / refresh-counter invalidation shared by the Workshop,
 * Collections and Author views.
 *
 * The three callers differ only in the command name, whether a collection is
 * open, how the cache key is scoped, and whether the next page is preloaded —
 * all explicit options here rather than divergent copy-pasted effects.
 */
export function useWorkshopPage({
  cmd,
  filters,
  enabled = true,
  cacheScope = null,
  preloadNext = true,
  mockKind,
}: UseWorkshopPageOptions): UseWorkshopPageResult {
  const steamPhase = useSteamSessionStore((s) => s.phase);
  const refreshCounter = useRefreshStore((s) => s.counter);
  const [page, setPage] = useState<WorkshopPage | null>(null);
  const [loading, setLoading] = useState(true);
  const cache = useRef<Map<string, WorkshopPage>>(new Map());

  const filtersKey = useMemo(() => JSON.stringify(filters), [filters]);

  // A forced refresh clears the preload cache: its pages may be stale.
  useEffect(() => {
    cache.current.clear();
  }, [refreshCounter]);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    const timer = setTimeout(() => {
      if (inTauri && (steamPhase === "idle" || steamPhase === "logging-in")) {
        setLoading(true);
        return;
      }

      const key = cacheScope ? `${filtersKey}|${cacheScope}` : filtersKey;
      const cached = cache.current.get(key);
      if (cached) {
        setPage(cached);
        setLoading(false);
      } else {
        setLoading(true);
      }

      void (async () => {
        if (!inTauri) {
          setPage(mockKind ? makeMockPage(mockKind, filters.page) : null);
          setLoading(false);
          return;
        }
        if (refreshCounter > 0) {
          await tryInvokeOk("workshop_refresh_cache");
        }
        let result = cached;
        if (!result) {
          result = (await tryInvoke<WorkshopPage>(cmd, { filters })) ?? undefined;
          if (result) cache.current.set(key, result);
        }
        if (!active) return;
        setPage(result ?? null);
        setLoading(false);

        if (preloadNext && result && result.total_pages > filters.page) {
          const preloadOn = await tryInvoke<boolean>(
            "config_get",
            { path: "settings.general.behavior.preload_next_page" },
            true,
          );
          if (preloadOn === false) return;
          const nextFilters: WorkshopFilters = {
            ...filters,
            page: filters.page + 1,
          };
          const nextKey = cacheScope
            ? `${JSON.stringify(nextFilters)}|${cacheScope}`
            : JSON.stringify(nextFilters);
          if (!cache.current.has(nextKey)) {
            void tryInvoke<WorkshopPage>(cmd, { filters: nextFilters }).then(
              (next) => {
                if (next) cache.current.set(nextKey, next);
              },
            );
          }
        }
      })();
    }, 0);

    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [enabled, cmd, filters, filtersKey, cacheScope, preloadNext, mockKind, refreshCounter, steamPhase]);

  return { page, loading, cache };
}
