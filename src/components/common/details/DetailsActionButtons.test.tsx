import { describe, expect, it, vi } from "vitest";
import { render, fireEvent, screen } from "@testing-library/react";
import DetailsActionButtons from "./DetailsActionButtons";
import { Meta } from "@/hooks/useDetailsMeta";
import { WorkshopItem } from "@/types/workshop";

vi.mock("@/lib/tauri", () => ({
  inTauri: false,
  tryInvoke: vi.fn().mockResolvedValue(null),
  tryInvokeOk: vi.fn().mockResolvedValue(true),
  tryInvokeAction: vi.fn().mockResolvedValue(true),
}));
vi.mock("@/stores/toasts", () => ({ pushToast: vi.fn() }));
vi.mock("@/lib/window", () => ({ maybeMinimize: vi.fn() }));
vi.mock("@/hooks/useConfirm", () => ({
  useConfirm: () => ({ confirm: vi.fn(), ConfirmDialog: null }),
}));
vi.mock("@/components/common/Tooltip", () => ({
  Tooltip: ({ children }: any) => <>{children}</>,
}));

const item: WorkshopItem = {
  pubfileid: "42",
  title: "Wallpaper",
  preview_url: "",
  author: "a",
  author_url: "",
  description: "",
  file_size: "",
  posted_date: "",
  updated_date: "",
  tags: [],
  rating_star_file: "",
  num_ratings: "",
  is_collection: false,
} as WorkshopItem;

const meta = (over: Partial<Meta> = {}): Meta => ({
  pubfileid: "42",
  title: "Wallpaper",
  preview: "",
  description: "",
  is_collection: false,
  ...over,
});

const baseProps = {
  kind: "workshop" as const,
  item,
  installedEntry: undefined,
  showInstalledActions: false,
  openWorkshopPage: vi.fn().mockResolvedValue(undefined),
  refreshInstalled: vi.fn().mockResolvedValue(undefined),
  removeOptimistic: vi.fn(),
  addOptimistic: vi.fn(),
  onClose: vi.fn(),
};

describe("DetailsActionButtons", () => {
  it("shows the install button for a workshop item", () => {
    render(<DetailsActionButtons {...baseProps} meta={meta()} />);
    // The global i18n mock resolves the key against the English bundle.
    expect(screen.getByText("Install wallpaper")).toBeInTheDocument();
  });

  it("calls onDownload and onClose when install is clicked", () => {
    const onDownload = vi.fn();
    const onClose = vi.fn();
    render(
      <DetailsActionButtons
        {...baseProps}
        meta={meta()}
        onDownload={onDownload}
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByText("Install wallpaper"));
    expect(onDownload).toHaveBeenCalledWith(item);
    expect(onClose).toHaveBeenCalled();
  });

  it("disables the install button for a collection", () => {
    render(
      <DetailsActionButtons {...baseProps} meta={meta({ is_collection: true })} />,
    );
    expect(screen.getByText("Install wallpaper").closest("button")).toBeDisabled();
  });

  it("keeps a harmless install button when no item is selected", () => {
    // The button renders regardless, but clicking it with a null item must not
    // fire the download callback.
    const onDownload = vi.fn();
    render(
      <DetailsActionButtons
        {...baseProps}
        item={null}
        meta={meta()}
        onDownload={onDownload}
      />,
    );
    fireEvent.click(screen.getByText("Install wallpaper"));
    expect(onDownload).not.toHaveBeenCalled();
  });
});
