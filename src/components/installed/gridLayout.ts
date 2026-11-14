/**
 * Layout arithmetic for the Installed grid.
 *
 * Kept in its own module so the component file exports only a component (the
 * `react-refresh/only-export-components` rule requires that) and so the
 * breakpoints can be unit-tested directly.
 */

/** Minimum width a card is allowed to shrink to before a column is dropped. */
export const MIN_COL_WIDTH = 190;

/** Horizontal gap between cards, in px. */
export const GRID_GAP = 12;

/**
 * How many columns fit into `width` px.
 *
 * Returns 0 when `width` is 0, which means "not measured yet" — callers must
 * treat that as "render nothing" rather than guessing a column count.
 *
 * The column count shifts by one near a boundary whenever the measured width
 * is off by even a few pixels, so the exact thresholds are pinned by tests.
 */
export function computeColumns(
  width: number,
  minColWidth = MIN_COL_WIDTH,
  gap = GRID_GAP,
): number {
  if (!Number.isFinite(width) || width <= 0) return 0;
  return Math.max(1, Math.floor((width + gap) / (minColWidth + gap)));
}
