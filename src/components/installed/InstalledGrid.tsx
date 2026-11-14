import { useRef, useState, useEffect, useLayoutEffect, useMemo } from "react";
import { InstalledWallpaper } from "@/types/workshop";
import { InstalledMetadata } from "@/types/workshop";
import { useAppStore } from "@/stores/app";
import { motion } from "framer-motion";
import WallpaperCard from "./WallpaperCard";
import { computeColumns, GRID_GAP, MIN_COL_WIDTH } from "./gridLayout";

interface InstalledGridProps {
  items: InstalledWallpaper[];
  selected: InstalledWallpaper | null;
  selectionMode: boolean;
  selectedIds: Set<string>;
  toggleSelection: (id: string) => void;
  setSelected: (item: InstalledWallpaper | null) => void;
  metaMap: Record<string, InstalledMetadata>;
  onApply: (item: InstalledWallpaper) => void;
  onExtract: (item: InstalledWallpaper) => void;
  onDelete: (item: InstalledWallpaper) => void;
  onOpenFolder: (item: InstalledWallpaper) => void;
  onCopyId: (item: InstalledWallpaper) => void;
}

export default function InstalledGrid({
  items,
  selected,
  selectionMode,
  selectedIds,
  toggleSelection,
  setSelected,
  metaMap,
  onApply,
  onExtract,
  onDelete,
  onOpenFolder,
  onCopyId,
}: InstalledGridProps) {
  const enableLayoutAnimations = useAppStore((s) => s.enableLayoutAnimations);
  const containerRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [dimensions, setDimensions] = useState({ width: 0, height: 0 });

  const gap = GRID_GAP;

  const { cols, colWidth, itemHeight, totalRows, totalHeight } = useMemo(() => {
    const measured = computeColumns(dimensions.width, MIN_COL_WIDTH, gap);
    // `measured === 0` means genuinely unmeasured (hidden container). Fall back
    // to a sane column count so the grid still renders; the value is replaced
    // as soon as either `useLayoutEffect` or the ResizeObserver reports a real
    // width. This also keeps non-layout environments (JSDOM tests) working,
    // where every element reports zero width.
    const c = measured === 0 ? 4 : measured;
    const usableWidth =
      measured === 0 ? c * MIN_COL_WIDTH + (c - 1) * gap : dimensions.width;
    const cw = (usableWidth - (c - 1) * gap) / c;
    const ih = cw;
    const tr = Math.ceil(items.length / c);
    const th = tr * ih + (tr > 0 ? (tr - 1) * gap : 0) + 72;
    return {
      cols: c,
      colWidth: cw,
      itemHeight: ih,
      totalRows: tr,
      totalHeight: th,
    };
  }, [dimensions.width, items.length, gap]);

  // Measure synchronously, before the browser paints.
  //
  // This used to run inside a plain `useEffect`, which fires *after* paint. The
  // first frame therefore rendered with `dimensions.width === 0` — a hard-coded
  // 4-column fallback — and only the follow-up render used the real width, so
  // the cards visibly snapped to a new layout on every visit.
  //
  // `useLayoutEffect` runs after the DOM is updated but before paint, so the
  // state update it schedules is folded into the same render and no
  // intermediate frame reaches the screen.
  //
  // CRITICAL: measure the *content box*, exactly like the ResizeObserver below.
  //
  // `clientWidth` is the **padding box** width: it includes the container's
  // `px-4` (32px in total). `ResizeObserver`'s `contentRect.width` is the
  // **content box** width and excludes it. Using `clientWidth` here made the
  // first frame believe the container was 32px wider than it is, which was
  // enough to cross a column breakpoint — the grid briefly showed one extra
  // column (e.g. 5) and then settled on the correct one (4). Do not "simplify"
  // this back to `el.clientWidth`.
  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const style = getComputedStyle(el);
    const padX =
      (parseFloat(style.paddingLeft) || 0) +
      (parseFloat(style.paddingRight) || 0);
    const padY =
      (parseFloat(style.paddingTop) || 0) +
      (parseFloat(style.paddingBottom) || 0);
    // `clientWidth` already excludes the scrollbar, so subtracting only the
    // padding yields precisely what `contentRect.width` reports.
    const width = el.clientWidth - padX;
    const height = el.clientHeight - padY;
    // A hidden container measures 0; leave the unmeasured state in place and
    // let the ResizeObserver below deliver the real size once it is visible.
    if (width <= 0 || height <= 0) return;
    setDimensions((prev) =>
      prev.width === width && prev.height === height
        ? prev
        : { width, height },
    );
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const handleScroll = () => {
      setScrollTop(el.scrollTop);
    };

    const ro = new ResizeObserver(([entry]) => {
      const { width, height } = entry.contentRect;
      setDimensions((prev) => {
        // Any width change matters — it changes the column count and every
        // card's size, so it must never be filtered out. Height changes are
        // mostly the toolbar expanding or collapsing, and reacting to those
        // would thrash the whole grid, so those keep a tolerance.
        //
        // Comparing width exactly (rather than with a tolerance) is safe here
        // because `contentRect` reports fractions consistently; it is the
        // *height* that jitters as rows resize.
        const widthChanged = prev.width !== width;
        const heightChanged = Math.abs(prev.height - height) >= 150;
        if (!widthChanged && !heightChanged) return prev;
        return { width, height };
      });
    });

    el.addEventListener("scroll", handleScroll, { passive: true });
    ro.observe(el);

    return () => {
      el.removeEventListener("scroll", handleScroll);
      ro.disconnect();
    };
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const maxScroll = Math.max(0, totalHeight - dimensions.height);
    if (el.scrollTop > maxScroll) {
      try {
        el.scrollTop = maxScroll;
      } catch {
        // JSDOM has read-only scrollTop
      }
      setScrollTop(maxScroll);
    }
  }, [items.length, totalHeight, dimensions.height]);

  const visibleItems = useMemo(() => {
    if (dimensions.height === 0 || items.length === 0) {
      // Height is unknown (unmeasured, or a non-layout environment such as
      // JSDOM). Render a fixed slice so the grid is never empty.
      return items.slice(0, 12).map((item, index) => ({ item, index }));
    }
    const buffer = 2;
    const startRow = Math.max(
      0,
      Math.floor(scrollTop / (itemHeight + gap)) - buffer,
    );
    const endRow = Math.min(
      totalRows,
      Math.ceil((scrollTop + dimensions.height) / (itemHeight + gap)) + buffer,
    );

    const result = [];
    const startIndex = startRow * cols;
    const endIndex = Math.min(items.length, endRow * cols);

    for (let i = startIndex; i < endIndex; i++) {
      result.push({ item: items[i], index: i });
    }
    return result;
  }, [items, scrollTop, dimensions.height, cols, totalRows, itemHeight, gap]);

  return (
    <div ref={containerRef} className="relative flex-1 overflow-auto px-4 pt-3 pb-20">
      <div
        style={{
          height: `${totalHeight}px`,
          width: "100%",
          position: "relative",
        }}
      >
        {visibleItems.map(({ item, index }) => {
          const row = Math.floor(index / cols);
          const col = index % cols;
          const left = col * (colWidth + gap);
          const top = row * (itemHeight + gap);

          return enableLayoutAnimations ? (
            <motion.div
              key={`anim-${item.pubfileid}`}
              initial={false}
              animate={{
                top,
                left,
                width: colWidth,
                height: itemHeight,
              }}
              transition={{ type: "spring", stiffness: 300, damping: 30 }}
              style={{
                position: "absolute",
              }}
            >
              <WallpaperCard
                item={item}
                isSelected={
                  selected?.pubfileid === item.pubfileid && !selectionMode
                }
                selectionMode={selectionMode}
                isBulkSelected={selectedIds.has(item.pubfileid)}
                onToggleBulk={() => toggleSelection(item.pubfileid)}
                onSelect={() => setSelected(item)}
                author={metaMap[item.pubfileid]?.author}
                onApply={onApply}
                onExtract={onExtract}
                onDelete={onDelete}
                onOpenFolder={onOpenFolder}
                onCopyId={onCopyId}
              />
            </motion.div>
          ) : (
            <div
              key={`static-${item.pubfileid}`}
              style={{
                position: "absolute",
                top: `${top}px`,
                left: `${left}px`,
                width: `${colWidth}px`,
                height: `${itemHeight}px`,
              }}
            >
              <WallpaperCard
                item={item}
                isSelected={
                  selected?.pubfileid === item.pubfileid && !selectionMode
                }
                selectionMode={selectionMode}
                isBulkSelected={selectedIds.has(item.pubfileid)}
                onToggleBulk={() => toggleSelection(item.pubfileid)}
                onSelect={() => setSelected(item)}
                author={metaMap[item.pubfileid]?.author}
                onApply={onApply}
                onExtract={onExtract}
                onDelete={onDelete}
                onOpenFolder={onOpenFolder}
                onCopyId={onCopyId}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
