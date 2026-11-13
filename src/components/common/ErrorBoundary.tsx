import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Shown instead of the crashed subtree. */
  fallback?: ReactNode;
  /** Label used in the console message, e.g. "PresetSettings". */
  label?: string;
}

interface State {
  error: Error | null;
}

/**
 * Catches render-time exceptions in a subtree.
 *
 * Without one, a single malformed value (a wallpaper whose `options` are
 * objects instead of strings, a colour string that does not parse, …) throws
 * during render, React unmounts the whole root, and the Tauri window ends up
 * completely black — indistinguishable from a GPU crash. That is exactly what
 * the 麻匪 wallpapers caused. Confining the failure to a small panel keeps the
 * rest of the UI usable and surfaces a readable message.
 */
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ErrorBoundary${this.props.label ? ":" + this.props.label : ""}]`, error, info);
  }

  render() {
    if (this.state.error) {
      if (this.props.fallback) return this.props.fallback;
      return (
        <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
          <p className="text-sm font-medium text-danger">渲染出错 / Render error</p>
          <p className="text-[11px] leading-relaxed break-all text-white/50">
            {this.state.error.message}
          </p>
        </div>
      );
    }
    return this.props.children;
  }
}
