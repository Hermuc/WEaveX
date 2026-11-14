import { describe, expect, it } from "vitest";
import { render, fireEvent } from "@testing-library/react";

import PreviewImage from "./PreviewImage";

function getImg(container: HTMLElement): HTMLImageElement {
  const img = container.querySelector("img");
  if (!img) throw new Error("expected an <img> to be rendered");
  return img as HTMLImageElement;
}

describe("PreviewImage", () => {
  it("never forwards a title into the img alt attribute", () => {
    // Regression: the grid passed each wallpaper's title as `alt`. A browser
    // paints alt text in place of the picture until the source decodes, so the
    // title appeared as an oversized second copy floating over the artwork.
    // The title already exists as a real heading, so the preview must be
    // decorative.
    const { container } = render(
      <PreviewImage src="/tmp/preview.jpg" alt="Some Wallpaper Title" />,
    );

    const img = getImg(container);
    expect(img.getAttribute("alt")).toBe("");
    expect(img.getAttribute("alt")).not.toContain("Some Wallpaper Title");
  });

  it("stays transparent until the image has loaded", () => {
    // While the file is still resolving nothing may show through -- not the
    // picture, not alt text, not a broken-image glyph.
    const { container } = render(<PreviewImage src="/tmp/preview.jpg" />);
    const img = getImg(container);

    expect(img.style.opacity).toBe("0");

    fireEvent.load(img);
    expect(img.style.opacity).toBe("1");
  });

  it("renders a text-free placeholder when the source fails", () => {
    const { container } = render(<PreviewImage src="/tmp/missing.jpg" />);
    fireEvent.error(getImg(container));

    // The failed branch swaps in an icon placeholder with no text at all.
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("renders a text-free placeholder when no src is given", () => {
    const { container } = render(<PreviewImage />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("resets the loaded flag when the source changes", () => {
    const { container, rerender } = render(<PreviewImage src="/tmp/a.jpg" />);
    fireEvent.load(getImg(container));
    expect(getImg(container).style.opacity).toBe("1");

    rerender(<PreviewImage src="/tmp/b.jpg" />);
    // A new source must go back to invisible, otherwise the previous image
    // would linger over the new one while it decodes.
    expect(getImg(container).style.opacity).toBe("0");
  });
});
