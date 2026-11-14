import { useEffect, useRef, useState } from "react";
import { useTranslation } from "@/i18n/hooks";

import FilterBar from "@/components/workshop/FilterBar";
import WorkshopFilterDrawer from "@/components/workshop/WorkshopFilterDrawer";
import Pagination from "@/components/workshop/Pagination";
import WorkshopCard from "@/components/workshop/WorkshopCard";
import WorkshopGrid from "@/components/workshop/WorkshopGrid";
import DetailsPanel from "@/components/common/DetailsPanel";
import { SkeletonCard } from "@/components/common/Skeleton";
import { useFiltersStore } from "@/stores/filters";
import { useAppStore } from "@/stores/app";
import { pushToast } from "@/stores/toasts";
import { usePaginationContext } from "@/hooks/usePaginationContext";
import { useWorkshopPage } from "@/hooks/useWorkshopPage";
import { describeDownloadError } from "@/lib/errors";
import { useWatchWhileDownloading } from "@/hooks/useWatchWhileDownloading";
import { inTauri, tryInvokeAction } from "@/lib/tauri";
import { WorkshopItem } from "@/types/workshop";

export default function WorkshopView() {
  const { t } = useTranslation();
  const filters = useFiltersStore((s) => s.filters);
  const showAdvanced = useFiltersStore((s) => s.showAdvanced);
  const setPage = useFiltersStore((s) => s.setPage);
  const setViewPage = useFiltersStore((s) => s.setViewPage);
  const getViewPage = useFiltersStore((s) => s.getViewPage);
  const accountIndex = useAppStore((s) => s.accountIndex);
  const [selected, setSelected] = useState<WorkshopItem | null>(null);

  // Get the saved page for workshop view
  const currentPage = getViewPage("workshop");

  // Sync filters.page with the view-specific page
  useEffect(() => {
    if (filters.page !== currentPage) {
      setPage(currentPage);
    }
  }, [currentPage, filters.page, setPage]);

  const scrollContainerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    scrollContainerRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  }, [filters.page]);

  const { page, loading } = useWorkshopPage({
    cmd: "workshop_browse",
    filters,
    mockKind: "workshop",
  });

  const items = page?.items ?? [];
  const total = page?.total_items ?? 0;
  const totalPages = page?.total_pages ?? 1;

  // Publish pagination context so global hotkeys (mouse back/forward, etc.)
  // can act on this view's pagination.
  const handlePageChange = (newPage: number) => {
    setPage(newPage);
    setViewPage("workshop", newPage);
  };
  usePaginationContext({
    view: "workshop",
    page: filters.page,
    totalPages,
    onPageChange: handlePageChange,
  });

  const { watch: handleWatch, state: watchState } = useWatchWhileDownloading();

  const handleDownload = async (item: WorkshopItem) => {
    if (item.is_collection) {
      setSelected(item);
      return;
    }
    if (!inTauri) {
      pushToast(t("messages.download_started"), "success");
      return;
    }
    const res = await tryInvokeAction("download_start", {
      pubfileid: item.pubfileid,
      accountIndex,
    });

    if (res.ok) {
      pushToast(t("messages.download_started"), "success");
    } else {
      pushToast(describeDownloadError(res.error, t), "error");
    }
  };

  return (
    <div className="flex h-full flex-col">
      <FilterBar />
      <div className="relative flex flex-1 overflow-hidden">
        <WorkshopFilterDrawer open={showAdvanced} />
        <div ref={scrollContainerRef} className="flex-1 overflow-auto px-4 pt-3 pb-20">
          {loading ? (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(190px,1fr))] gap-3">
              {Array.from({ length: 30 }).map((_, i) => (
                <SkeletonCard key={i} />
              ))}
            </div>
          ) : items.length === 0 ? (
            <div className="flex h-64 items-center justify-center text-sm text-muted">
              {t("labels.no_wallpapers_found")}
            </div>
          ) : (
            <WorkshopGrid
              items={items}
              containerRef={scrollContainerRef}
              renderItem={(item) => (
                <WorkshopCard
                  key={item.pubfileid}
                  item={item}
                  onOpen={setSelected}
                  onDownload={handleDownload}

                />
              )}
            />
          )}
        </div>
      </div>
      <Pagination
        page={filters.page}
        totalPages={totalPages}
        onChange={(newPage) => {
          setPage(newPage);
          setViewPage("workshop", newPage);
        }}
        infoText={
          total
            ? t("labels.showing_wallpapers", {
                start: (filters.page - 1) * 30 + 1,
                end: (filters.page - 1) * 30 + items.length,
                total,
              })
            : ""
        }
      />
      <DetailsPanel
        kind="workshop"
        item={selected}
        onClose={() => setSelected(null)}
        onDownload={handleDownload}

        onWatch={handleWatch}
        watchState={watchState}
      />
    </div>
  );
}
