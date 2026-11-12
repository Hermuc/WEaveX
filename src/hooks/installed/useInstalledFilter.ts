import { useMemo, useState } from "react";
import { useTasksStore } from "@/stores/tasks";
import { useFiltersStore } from "@/stores/filters";
import { extractTagLabel, parseRatingStars } from "@/lib/workshop";
import {
  MISC_TAG_KEYS,
  GENRE_TAG_KEYS,
  type LocalSortKey,
} from "@/lib/filterConfig";
import { InstalledWallpaper, InstalledMetadata } from "@/types/workshop";

interface UseInstalledFilterArgs {
  items: InstalledWallpaper[];
  metaMap: Record<string, InstalledMetadata>;
}

/**
 * Owns every piece of *filtering / sorting* state for the Installed view:
 * the raw filter inputs, derived tag/author facets, and the memoised
 * `filtered` result. Extracted from `useWallpaperActions` so the latter
 * only has to orchestrate.
 */
export function useInstalledFilter({ items, metaMap }: UseInstalledFilterArgs) {
  const [search, setSearch] = useState("");
  const [category, setCategory] = useState<string>("");
  const [typeFilter, setTypeFilter] = useState<string>("");
  // Default to "Everyone" rather than "Any": the Installed view should open on
  // a safe filter, and users who want mature content can switch explicitly.
  const [age, setAge] = useState<string>("Everyone");
  const [resolution, setResolution] = useState<string>("");
  const [tagFilters, setTagFilters] = useState<string[]>([]);
  const [excludedTagFilters, setExcludedTagFilters] = useState<string[]>([]);
  const [authorFilters, setAuthorFilters] = useState<string[]>([]);
  const [excludedAuthorFilters, setExcludedAuthorFilters] = useState<string[]>(
    [],
  );
  const [sort, setSort] = useState<LocalSortKey>("install_date");
  const [sortOrder, setSortOrder] = useState<"asc" | "desc">("desc");
  const showAdvanced = useFiltersStore((s) => s.showInstalledAdvanced);
  const setShowAdvanced = useFiltersStore((s) => s.setShowInstalledAdvanced);
  const downloadTasks = useTasksStore((s) => s.tasks);

  const tagsFor = useMemo(
    () =>
      (item: InstalledWallpaper): Set<string> => {
        const set = new Set<string>(item.tags);
        // Wallpapers carry their Steam content rating in project.json rather
        // than as a tag, so fold it in here: the Age Rating filter matches on
        // this set, and without it a rated wallpaper only showed up under "Any".
        if (item.content_rating) set.add(item.content_rating);
        const raw = metaMap[item.pubfileid]?.tags;
        if (Array.isArray(raw)) {
          for (const tag of raw) {
            const label =
              typeof tag === "string"
                ? tag
                : typeof tag === "object" && tag && "tag" in tag
                  ? extractTagLabel(tag as { tag?: string })
                  : "";
            if (label) set.add(label);
          }
        }
        return set;
      },
    [metaMap],
  );

  const metaFor = useMemo(
    () => (item: InstalledWallpaper) => metaMap[item.pubfileid],
    [metaMap],
  );

  const presentTags = useMemo(() => {
    const set = new Set<string>();
    for (const it of items) {
      for (const tag of tagsFor(it)) set.add(tag);
    }
    return set;
  }, [items, tagsFor]);

  const visibleMiscTags = useMemo(
    () => MISC_TAG_KEYS.filter((k) => presentTags.has(k)),
    [presentTags],
  );
  const visibleGenreTags = useMemo(
    () => GENRE_TAG_KEYS.filter((k) => presentTags.has(k)),
    [presentTags],
  );

  const visibleAuthors = useMemo(() => {
    const authorSet = new Set<string>();
    for (const item of items) {
      const author = metaMap[item.pubfileid]?.author;
      if (author && author.trim()) {
        authorSet.add(author.trim());
      }
    }
    return Array.from(authorSet).sort((a, b) =>
      a.localeCompare(b, undefined, { sensitivity: "base" }),
    );
  }, [items, metaMap]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();

    const downloadingIds = new Set(
      Object.entries(downloadTasks)
        .filter(
          ([key, task]) =>
            key.startsWith("download:") &&
            (task.phase === "starting" || task.phase === "running"),
        )
        .map(([key]) => key.replace("download:", "")),
    );

    let result = items.filter((item) => {
      if (downloadingIds.has(item.pubfileid)) return false;

      if (q) {
        const inTitle = item.title.toLowerCase().includes(q);
        const inId = item.pubfileid.includes(q);
        if (!inTitle && !inId) return false;
      }
      const has = tagsFor(item);
      if (category && !has.has(category)) return false;
      if (typeFilter) {
        const ft = item.file_type?.toLowerCase() ?? "";
        const want = typeFilter.toLowerCase();
        if (!has.has(typeFilter) && ft !== want) return false;
      }
      if (age) {
        // "Everyone" also covers wallpapers that declare no rating at all:
        // Wallpaper Engine sample projects and older uploads omit
        // contentrating, and treating those as non-Everyone hid them from the
        // default view entirely.
        const rated = has.has(age);
        const unrated =
          age === "Everyone" &&
          !has.has("Everyone") &&
          !has.has("Questionable") &&
          !has.has("Mature");
        if (!rated && !unrated) return false;
      }
      if (resolution && !has.has(resolution)) return false;
      if (tagFilters.length > 0) {
        if (!tagFilters.every((f) => has.has(f))) return false;
      }
      if (excludedTagFilters.length > 0) {
        if (excludedTagFilters.some((f) => has.has(f))) return false;
      }
      if (authorFilters.length > 0 || excludedAuthorFilters.length > 0) {
        const author = metaMap[item.pubfileid]?.author?.trim() || "";
        if (authorFilters.length > 0 && !authorFilters.includes(author))
          return false;
        if (
          excludedAuthorFilters.length > 0 &&
          excludedAuthorFilters.includes(author)
        )
          return false;
      }
      return true;
    });

    const dir = sortOrder === "asc" ? 1 : -1;
    const ratingScore = (it: InstalledWallpaper) => {
      const star = metaFor(it)?.rating_star_file ?? "";
      return parseRatingStars(star);
    };
    const dateMs = (s?: string) => {
      if (!s) return 0;
      const t = Date.parse(s);
      return Number.isFinite(t) ? t : 0;
    };

    result = [...result].sort((a, b) => {
      switch (sort) {
        case "install_date":
          return (a.installed_ts - b.installed_ts) * dir;
        case "name":
          return (
            a.title.localeCompare(b.title, undefined, { sensitivity: "base" }) *
            dir
          );
        case "rating":
          return (ratingScore(a) - ratingScore(b)) * dir;
        case "size":
          return (a.size_bytes - b.size_bytes) * dir;
        case "posted_date":
          return (
            (dateMs(metaFor(a)?.posted_date) -
              dateMs(metaFor(b)?.posted_date)) *
            dir
          );
        case "updated_date":
          return (
            (dateMs(metaFor(a)?.updated_date) -
              dateMs(metaFor(b)?.updated_date)) *
            dir
          );
        default:
          return 0;
      }
    });
    return result;
  }, [
    items,
    search,
    category,
    typeFilter,
    age,
    resolution,
    tagFilters,
    sort,
    sortOrder,
    excludedTagFilters,
    authorFilters,
    excludedAuthorFilters,
    tagsFor,
    metaFor,
    metaMap,
    downloadTasks,
  ]);

  const toggleTag = (tag: string) => {
    const isIncluded = tagFilters.includes(tag);
    const isExcluded = excludedTagFilters.includes(tag);
    if (isExcluded) {
      setExcludedTagFilters((prev) => prev.filter((t) => t !== tag));
    } else if (isIncluded) {
      setTagFilters((prev) => prev.filter((t) => t !== tag));
      setExcludedTagFilters((prev) => [...prev, tag]);
    } else {
      setTagFilters((prev) => [...prev, tag]);
    }
  };

  const toggleAuthor = (author: string) => {
    const isIncluded = authorFilters.includes(author);
    const isExcluded = excludedAuthorFilters.includes(author);
    if (isExcluded) {
      setExcludedAuthorFilters((prev) => prev.filter((a) => a !== author));
    } else if (isIncluded) {
      setAuthorFilters((prev) => prev.filter((a) => a !== author));
      setExcludedAuthorFilters((prev) => [...prev, author]);
    } else {
      setAuthorFilters((prev) => [...prev, author]);
    }
  };

  const totalSize = filtered.reduce((sum, i) => sum + i.size_bytes, 0);
  const hasActiveFilters =
    tagFilters.length > 0 ||
    excludedTagFilters.length > 0 ||
    authorFilters.length > 0 ||
    excludedAuthorFilters.length > 0 ||
    category !== "" ||
    typeFilter !== "" ||
    age !== "" ||
    resolution !== "" ||
    search.length > 0;
  const hasAnyExtraTags =
    visibleAuthors.length > 0 ||
    visibleMiscTags.length > 0 ||
    visibleGenreTags.length > 0;

  const activeFiltersCount = [
    category !== "",
    typeFilter !== "",
    age !== "",
    resolution !== "",
    tagFilters.length > 0,
    excludedTagFilters.length > 0,
  ].filter(Boolean).length;

  return {
    search,
    setSearch,
    category,
    setCategory,
    typeFilter,
    setTypeFilter,
    age,
    setAge,
    resolution,
    setResolution,
    tagFilters,
    setTagFilters,
    excludedTagFilters,
    setExcludedTagFilters,
    authorFilters,
    setAuthorFilters,
    excludedAuthorFilters,
    setExcludedAuthorFilters,
    sort,
    setSort,
    sortOrder,
    setSortOrder,
    showAdvanced,
    setShowAdvanced,
    filtered,
    tagsFor,
    metaFor,
    presentTags,
    visibleMiscTags,
    visibleGenreTags,
    visibleAuthors,
    totalSize,
    hasActiveFilters,
    hasAnyExtraTags,
    activeFiltersCount,
    toggleTag,
    toggleAuthor,
  };
}
