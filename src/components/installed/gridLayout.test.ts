import { describe, expect, it } from "vitest";

import { computeColumns, GRID_GAP, MIN_COL_WIDTH } from "./gridLayout";

describe("computeColumns", () => {
  it("returns 0 before the container has been measured", () => {
    // 0 means "unknown", not "one column". The grid uses it to render nothing
    // rather than falling back to a guessed layout.
    expect(computeColumns(0)).toBe(0);
    expect(computeColumns(-1)).toBe(0);
    expect(computeColumns(Number.NaN)).toBe(0);
  });

  it("never returns fewer than one column for a real width", () => {
    expect(computeColumns(1)).toBe(1);
    expect(computeColumns(100)).toBe(1);
    expect(computeColumns(MIN_COL_WIDTH)).toBe(1);
  });

  it("pins the 4/5 column boundary", () => {
    // This is the boundary the first-frame bug landed on. Measuring the
    // container's padding box (32px wider than its content box) pushed a
    // 966..997px content area over the edge and produced 5 columns instead of
    // 4. These exact values are the regression lock.
    expect(computeColumns(966)).toBe(4);
    expect(computeColumns(997)).toBe(4);
    expect(computeColumns(998)).toBe(5);
  });

  it("pins every column boundary", () => {
    // floor((width + 12) / 202): the nth column needs width >= n*202 - 12.
    //   2 -> 392   3 -> 594   4 -> 796   5 -> 998   6 -> 1200   7 -> 1402
    const boundaries: [number, number][] = [
      [391, 1],
      [392, 2],
      [593, 2],
      [594, 3],
      [795, 3],
      [796, 4],
      [997, 4],
      [998, 5],
      [1199, 5],
      [1200, 6],
      [1401, 6],
      [1402, 7],
    ];
    for (const [width, expected] of boundaries) {
      expect(computeColumns(width), `width ${width}`).toBe(expected);
    }
  });

  it("would have produced one extra column with the old padding-box width", () => {
    // The bug in one line: adding the container's 32px of horizontal padding
    // to a 970px content area crosses into the 5-column band.
    const contentWidth = 970;
    const withPadding = contentWidth + 32;
    expect(computeColumns(contentWidth)).toBe(4);
    expect(computeColumns(withPadding)).toBe(5);
  });

  it("accepts custom layout constants", () => {
    // 1000px with 200px minimums and 0 gap fits exactly 5.
    expect(computeColumns(1000, 200, 0)).toBe(5);
    expect(computeColumns(999, 200, 0)).toBe(4);
  });

  it("uses the shared defaults", () => {
    expect(MIN_COL_WIDTH).toBe(190);
    expect(GRID_GAP).toBe(12);
    expect(computeColumns(MIN_COL_WIDTH * 4 + GRID_GAP * 3)).toBe(4);
  });
});
