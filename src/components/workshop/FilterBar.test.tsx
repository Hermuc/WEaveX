import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, fireEvent, screen } from "@testing-library/react";
import FilterBar from "./FilterBar";
import { useFiltersStore, DEFAULT_FILTERS } from "@/stores/filters";

vi.mock("@/stores/app", () => ({
  useAppStore: (sel: any) => sel({ showActiveFilters: true }),
}));

vi.mock("@/components/common/Select", () => ({
  default: ({ value, onChange }: any) => (
    <select
      data-testid="select"
      value={value}
      onChange={(e) => onChange?.(e.target.value)}
    />
  ),
}));

vi.mock("@/components/common/Tooltip", () => ({
  Tooltip: ({ children }: any) => <>{children}</>,
}));

vi.mock("./SearchOptionsPopover", () => ({
  default: () => null,
}));

describe("FilterBar", () => {
  beforeEach(() => {
    useFiltersStore.setState({
      filters: { ...DEFAULT_FILTERS },
      showAdvanced: false,
    });
  });

  it("renders the search input", () => {
    render(<FilterBar />);
    // The global i18n mock resolves keys against the real English bundle.
    const input = screen.getByPlaceholderText("Search...");
    expect(input).toBeInTheDocument();
  });

  it("typing in the search box updates the filters store after debounce", async () => {
    vi.useFakeTimers();
    render(<FilterBar />);
    const input = screen.getByPlaceholderText("Search...");
    fireEvent.change(input, { target: { value: "anime" } });
    vi.advanceTimersByTime(500);
    vi.useRealTimers();
    expect(useFiltersStore.getState().filters.search).toBe("anime");
    expect(useFiltersStore.getState().filters.page).toBe(1);
  });

  it("toggles the advanced drawer button state", () => {
    render(<FilterBar />);
    const toggle = screen.getByLabelText("Filters");
    fireEvent.click(toggle);
    expect(useFiltersStore.getState().showAdvanced).toBe(true);
  });
});
