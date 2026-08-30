// Tauri commands are the only bridge between the frontend and spacewise-core.
// They must stay thin: no scanning/classification/safety logic lives here,
// it all lives in spacewise-core per docs/ARCHITECTURE.md.

#[tauri::command]
fn core_status() -> spacewise_core::CoreStatus {
    spacewise_core::status()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![core_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
