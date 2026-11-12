import { useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Image as ImageIcon } from "lucide-react";

import { inTauri } from "@/lib/tauri";
import { cn } from "@/lib/utils";

interface Props {
  src?: string;
  alt?: string;
  className?: string;
  fallback?: React.ReactNode;
  fit?: "cover" | "contain";
}

function isRemote(src: string): boolean {
  return /^(https?:|data:|blob:|asset:|tauri:)/i.test(src);
}

function toDisplaySrc(src?: string): string | undefined {
  if (!src) return undefined;
  if (isRemote(src)) return src;
  // Local absolute filesystem path -> convert via Tauri's asset protocol.
  if (inTauri) {
    try {
      return convertFileSrc(src);
    } catch {
      return src;
    }
  }
  return src;
}

/** Animated previews are expensive to decode, so only on-screen ones animate. */
function isAnimated(src: string): boolean {
  return /\.gif(\?|$)/i.test(src);
}

export default function PreviewImage({
  src,
  alt,
  className,
  fallback,
  fit = "cover",
}: Props) {
  const [prevSrc, setPrevSrc] = useState(src);
  const [resolved, setResolved] = useState<string | undefined>(
    toDisplaySrc(src),
  );
  const [failed, setFailed] = useState(false);
  const [visible, setVisible] = useState(false);
  const imgRef = useRef<HTMLImageElement | null>(null);

  if (src !== prevSrc) {
    setPrevSrc(src);
    setFailed(false);
    setResolved(toDisplaySrc(src));
  }

  // Wallpaper previews are commonly ~1MB animated GIFs and a grid can hold
  // dozens. Decoding them all at once froze the view on first paint, and
  // loading="lazy" does not help for local files (they resolve instantly, so
  // the decode still happens). Only animated previews are gated here; static
  // JPEGs are cheap enough to load eagerly.
  const animated = !!src && isAnimated(src);
  useEffect(() => {
    if (!animated) return;
    const el = imgRef.current;
    if (!el) return;
    if (typeof IntersectionObserver === "undefined") {
      // No observer (old runtime / tests): show immediately, deferring the
      // state write to a microtask keeps it out of the effect body.
      void Promise.resolve().then(() => setVisible(true));
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            setVisible(true);
            io.disconnect();
            return;
          }
        }
      },
      { rootMargin: "200px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [animated, resolved]);

  if (!src || failed) {
    return (
      <div
        className={cn(
          "flex size-full items-center justify-center bg-surface-sunken text-subtle",
          className,
        )}
      >
        {fallback ?? <ImageIcon className="size-6" />}
      </div>
    );
  }

  // Off-screen animated previews render a still placeholder rather than the
  // GIF itself, so no decode happens until the card is actually visible.
  const showSrc = animated && !visible ? undefined : resolved;

  return (
    <img
      ref={imgRef}
      src={showSrc}
      alt={alt ?? ""}
      draggable={false}
      loading="lazy"
      decoding="async"
      onError={() => {
        setFailed(true);
      }}
      className={cn(
        "size-full animate-fade-in",
        fit === "cover" ? "object-cover" : "object-contain",
        !showSrc && "bg-surface-sunken",
        className,
      )}
    />
  );
}