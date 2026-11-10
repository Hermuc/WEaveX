import { AnimatePresence, motion } from "framer-motion";
import { Trans } from "react-i18next";
import { useTranslation } from "@/i18n/hooks";
import { cn } from "@/lib/utils";
import { useFiltersStore } from "@/stores/filters";

const SEARCH_MODES = [
  { value: 0, labelKey: "filters.search_fields.title_and_description" },
  { value: 1, labelKey: "filters.search_fields.title_only" },
  { value: 2, labelKey: "filters.search_fields.description_only" },
] as const;

interface Props {
  open: boolean;
  onClose?: () => void;
}

export default function SearchOptionsPopover({ open }: Props) {
  const { t } = useTranslation();
  const searchMode = useFiltersStore((s) => s.filters.search_text_mode ?? 0);
  const setFilters = useFiltersStore((s) => s.setFilters);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0, y: 4, scale: 0.98 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 4, scale: 0.98 }}
          transition={{ duration: 0.15, ease: "easeOut" }}
          className="absolute top-full left-0 z-50 mt-2 w-full min-w-[340px] rounded-xl border border-border/80 bg-surface-raised/95 p-3.5 text-xs shadow-2xl backdrop-blur-2xl"
        >
          {/* Field Selection Title & Close Hint */}
          <div className="mb-2 flex items-center justify-between">
            <span className="text-[11px] font-medium text-subtle">
              {t("filters.search_fields.title")}
            </span>
            <span className="text-[10px] text-muted">
              {t("filters.search_fields.esc_to_close")}
            </span>
          </div>

          {/* Segmented Control / Tabs */}
          <div className="grid grid-cols-3 gap-1 rounded-lg border border-border/60 bg-surface-sunken p-1">
            {SEARCH_MODES.map((mode) => {
              const isSelected = searchMode === mode.value;
              return (
                <button
                  type="button"
                  key={mode.value}
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => {
                    setFilters({ search_text_mode: mode.value, page: 1 });
                  }}
                  className={cn(
                    "flex cursor-pointer items-center justify-center rounded-md p-1.5 text-center text-[11px] leading-tight font-medium transition-all select-none",
                    isSelected
                      ? "border border-border/80 bg-surface-raised font-semibold text-foreground shadow-xs"
                      : "text-subtle hover:bg-surface-raised/50 hover:text-foreground",
                  )}
                >
                  {t(mode.labelKey)}
                </button>
              );
            })}
          </div>

          {/* Syntax Help Section */}
          <div className="mt-3 flex flex-col gap-2.5 border-t border-border/40 pt-2.5 text-[11px] leading-relaxed text-subtle">
            <p className="text-subtle/90">
              <Trans
                i18nKey="filters.search_fields.help_text"
                components={{ b: <span className="font-semibold text-foreground" /> }}
              />
            </p>

            <ul className="flex flex-col gap-1.5 pl-0.5 text-muted">
              <li className="flex items-center gap-2">
                <code className="shrink-0 rounded border border-border/50 bg-surface-sunken px-1.5 py-0.5 font-mono text-[10px] font-semibold text-foreground">
                  {t("filters.search_fields.example_and")}
                </code>
                <span className="text-[10.5px] leading-tight text-subtle">
                  {t("filters.search_fields.help_example_and")}
                </span>
              </li>
              <li className="flex items-center gap-2">
                <code className="shrink-0 rounded border border-border/50 bg-surface-sunken px-1.5 py-0.5 font-mono text-[10px] font-semibold text-foreground">
                  {t("filters.search_fields.example_or")}
                </code>
                <span className="text-[10.5px] leading-tight text-subtle">
                  {t("filters.search_fields.help_example_or")}
                </span>
              </li>
              <li className="flex items-center gap-2">
                <code className="shrink-0 rounded border border-border/50 bg-surface-sunken px-1.5 py-0.5 font-mono text-[10px] font-semibold text-foreground">
                  {t("filters.search_fields.example_not")}
                </code>
                <span className="text-[10.5px] leading-tight text-subtle">
                  {t("filters.search_fields.help_example_not")}
                </span>
              </li>
              <li className="flex items-center gap-2">
                <code className="shrink-0 rounded border border-border/50 bg-surface-sunken px-1.5 py-0.5 font-mono text-[10px] font-semibold text-foreground">
                  {t("filters.search_fields.example_group")}
                </code>
                <span className="text-[10.5px] leading-tight text-subtle">
                  {t("filters.search_fields.grouping_example_1")}
                </span>
              </li>
            </ul>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
