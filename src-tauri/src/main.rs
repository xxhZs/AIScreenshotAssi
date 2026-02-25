// Hide the console window on Windows release builds (no-op on macOS).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod interceptor;
mod memory_engine;

#[tauri::command]
fn query_memory(query: String) -> String {
    memory_engine::query_memory(&query)
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![query_memory])
        .setup(|app| {
            let handle = app.handle().clone();
            interceptor::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("fatal: Darling failed to start");
}
