/**
 * Central registry of Tauri event channel names.
 *
 * Keeping every channel name in one place avoids typos between the Rust
 * emitter (`app_handle.emit("...", ...)`) and the frontend listener
 * (`listen("...", ...)`) and makes it easy to audit who listens to what.
 *
 * IMPORTANT: the string values must stay byte-for-byte identical to the
 * names emitted by the Rust backend. Do not rename them here alone.
 */
export const EVENTS = {
  // --- dotnet / plugin bootstrap status ---
  DOTNET_STATUS: "dotnet://status",
  PLUGIN_STATUS: "plugin://status",

  // --- download pipeline ---
  DOWNLOAD_STATUS: "download://status",
  DOWNLOAD_REQUIRE_2FA: "download://require_2fa",
  DOWNLOAD_REQUIRE_APP_CONFIRM: "download://require_app_confirm",

  // --- extract pipeline ---
  EXTRACT_STATUS: "extract://status",

  // --- metadata batch init ---
  METADATA_INIT_PROGRESS: "metadata-init-progress",

  // --- auth (DepotDownloader account flow) ---
  AUTH_REQUIRE_2FA: "auth://require_2fa",
  AUTH_REQUIRE_APP_CONFIRM: "auth://require_app_confirm",
  AUTH_SUCCESS: "auth://success",
  AUTH_FAILED: "auth://failed",
  AUTH_DONE: "auth://done",

  // --- Steam webview session ---
  STEAM_LOGIN_SUCCESS: "steam-login-success",
} as const;
