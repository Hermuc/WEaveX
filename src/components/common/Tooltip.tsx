import * as React from "react";
import * as RadixTooltip from "@radix-ui/react-tooltip";

interface TooltipProps {
  children: React.ReactNode;
  content: string;
  side?: "top" | "bottom" | "left" | "right";
  /** Kept for API compatibility; the shared provider owns the delay. */
  delay?: number;
}

/**
 * One provider for the whole app.
 *
 * Previously every Tooltip wrapped itself in its own `Provider`, so a view with
 * five tooltips created five independent contexts and five portal containers.
 * Mounting them together on a view switch produced visible flashes of empty
 * overlays. A single provider removes both the duplication and the flicker.
 */
export function TooltipProvider({
  children,
  delay = 250,
}: {
  children: React.ReactNode;
  delay?: number;
}) {
  return (
    <RadixTooltip.Provider delayDuration={delay}>
      {children}
    </RadixTooltip.Provider>
  );
}

/**
 * Tooltip that works with or without a surrounding provider.
 *
 * Radix throws when a Root is rendered outside a Provider. Rather than force
 * every test and isolated component to wrap itself, this falls back to its own
 * provider when none is present — the app always supplies one, so the fallback
 * only affects isolated renders.
 */
export function Tooltip({ children, content, side = "top" }: TooltipProps) {
  // Hooks must run unconditionally, so the context read comes first.
  const hasProvider = React.useContext(TooltipProviderContext);

  if (!content) return <>{children}</>;

  const root = (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          sideOffset={8}
          className="pointer-events-none z-99999 duration-200 animate-in fade-in zoom-in-95 data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=closed]:zoom-out-95"
        >
          <div className="relative drop-shadow-[0_4px_16px_rgba(0,0,0,0.8)]">
            <div className="rounded-md bg-surface-raised/95 px-3 py-1.5 text-xs font-medium text-foreground backdrop-blur-sm">
              {content}
            </div>
            <RadixTooltip.Arrow className="fill-surface-raised/95" />
          </div>
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );

  // Radix requires a Provider ancestor; supply a throwaway one when rendered in
  // isolation (unit tests, Storybook). The app-level provider wins otherwise.
  if (hasProvider) return root;
  return (
    <RadixTooltip.Provider delayDuration={250}>{root}</RadixTooltip.Provider>
  );
}

/** Tracks whether an app-level provider is present. */
const TooltipProviderContext = React.createContext(false);

const BaseTooltipProvider = TooltipProvider;

/** Provider that also marks the context so Tooltip can skip its fallback. */
export function AppTooltipProvider({
  children,
  delay = 250,
}: {
  children: React.ReactNode;
  delay?: number;
}) {
  return (
    <TooltipProviderContext.Provider value={true}>
      <BaseTooltipProvider delay={delay}>{children}</BaseTooltipProvider>
    </TooltipProviderContext.Provider>
  );
}