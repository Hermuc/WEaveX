use tauri::command;

use crate::commands::AppStateHandle;

#[command]
pub fn dotnet_init(state: AppStateHandle<'_>) {
    state.init_dotnet_runtime_async();
}
