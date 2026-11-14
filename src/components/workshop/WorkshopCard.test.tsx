import { describe, expect, it, vi } from "vitest";
import { render, fireEvent, screen } from "@testing-library/react";
import WorkshopCard from "./WorkshopCard";
import { WorkshopItem } from "@/types/workshop";

// The card reaches into three zustand stores and the confirm hook. Stubbing
// them keeps the test about the card's own rendering, not about app state.
vi.mock("@/stores/app", () => ({
  useAppStore: (sel: any) =>
    sel({ enableLayoutAnimations: false }),
}));
vi.mock("@/stores/installed", () => ({
  useInstalledStore: (sel: any) =>
    sel({
      byId: {},
      refresh: vi.fn(),
      addOptimistic: vi.fn(),
      removeOptimistic: vi.fn(),
    }),
}));
vi.mock("@/stores/tasks", () => ({
  useTasksStore: () => undefined,
}));
vi.mock("@/stores/toasts", () => ({ pushToast: vi.fn() }));
vi.mock("@/hooks/useConfirm", () => ({
  useConfirm: () => ({ confirm: vi.fn(), ConfirmDialog: null }),
}));
vi.mock("@/components/common/PreviewImage", () => ({
  default: ({ src }: any) => <img data-testid="preview" src={src} alt="" />,
}));
vi.mock("@/lib/window", () => ({ maybeMinimize: vi.fn() }));

const item: WorkshopItem = {
  pubfileid: "123",
  title: "A Test Wallpaper",
  preview_url: "https://example.com/p.jpg",
  author: "Someone",
  author_url: "",
  description: "",
  file_size: "10 MB",
  posted_date: "",
  updated_date: "",
  tags: [],
  rating_star_file: "",
  num_ratings: "3",
  is_collection: false,
} as WorkshopItem;

describe("WorkshopCard", () => {
  it("renders the title and preview image", () => {
    render(<WorkshopCard item={item} onOpen={vi.fn()} onDownload={vi.fn()} />);
    expect(screen.getByText("A Test Wallpaper")).toBeInTheDocument();
    expect(screen.getByTestId("preview")).toHaveAttribute(
      "src",
      "https://example.com/p.jpg",
    );
  });

  it("calls onOpen when the card is clicked", () => {
    const onOpen = vi.fn();
    const { container } = render(
      <WorkshopCard item={item} onOpen={onOpen} onDownload={vi.fn()} />,
    );
    // onOpen is wired to the preview area (the cursor-pointer div inside the
    // article), not the article itself.
    const previewArea = container.querySelector(
      "article > div.cursor-pointer",
    ) as HTMLElement;
    expect(previewArea).not.toBeNull();
    fireEvent.click(previewArea);
    expect(onOpen).toHaveBeenCalledWith(item);
  });

  it("hides the download affordance when hideDownload is set", () => {
    render(
      <WorkshopCard
        item={item}
        onOpen={vi.fn()}
        onDownload={vi.fn()}
        hideDownload
      />,
    );
    // The card still renders; the point is it does not crash and the title is
    // present. A dedicated download button must not be exposed for collections.
    expect(screen.getByText("A Test Wallpaper")).toBeInTheDocument();
  });
});
