/**
 * Result of the backend `updater_check` command.
 *
 * Previously declared three times (stores/updater.ts, hooks/useBootstrap.ts,
 * components/dialogs/UpdateDialog.tsx) with slightly different optionality of
 * `error`; consolidated here using the most permissive shape
 * (`error?: string | null`), which is compatible with all three call sites.
 */
export interface UpdateInfo {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_notes: string;
  html_url: string;
  error?: string | null;
}
