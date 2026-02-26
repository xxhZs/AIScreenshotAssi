// Hide the console window on Windows release builds (no-op on macOS).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod interceptor;
mod brain;
mod llm;
mod memory_engine;
mod vision;

fn load_dotenv() {
    // Best-effort: load `.env` so local dev config works without exporting env vars.
    // - `dotenv()` searches upward from the current dir in most setups.
    // - We also try `<repo>/.env` explicitly via `CARGO_MANIFEST_DIR/..` for stability.
    let _ = dotenvy::dotenv();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo_root) = manifest_dir.parent() {
        let _ = dotenvy::from_path(repo_root.join(".env"));
    }
}

#[tauri::command]
fn query_memory(query: String) -> String {
    memory_engine::query_memory(&query)
}

#[tauri::command]
async fn llm_prompt(prompt: String) -> Result<String, String> {
    llm::prompt_from_env(prompt).await.map_err(|e| e.user())
}

#[tauri::command]
async fn llm_chat(request: llm::LlmChatRequest) -> Result<llm::LlmChatResponse, String> {
    llm::chat(request).await.map_err(|e| e.user())
}

#[tauri::command]
async fn brain_run(request: brain::BrainRequest) -> Result<brain::BrainResponse, String> {
    brain::run(request).await
}

fn main() {
    load_dotenv();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            query_memory,
            llm_prompt,
            llm_chat,
            brain_run
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            interceptor::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("fatal: Darling failed to start");
}
