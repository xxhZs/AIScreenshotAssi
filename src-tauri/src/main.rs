// Hide the console window on Windows release builds (no-op on macOS).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod interceptor;
mod brain;
mod runlog;
mod policy;
mod runtime;
mod tool_server;

use tauri::Emitter;

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
async fn brain_run(
    app: tauri::AppHandle,
    request: brain::BrainRequest,
) -> Result<brain::BrainResponse, String> {
    let ctx = interceptor::last_context_snapshot();
    let run = runlog::start_run(&app, &request.input, ctx.as_ref())?;
    let hard_policy = policy::HardPolicy::load(&app);
    let _ = runlog::write_hard_policy(&run, &hard_policy);
    let mcp_server_command = tool_server::mcp_server_command();
    let result = runtime::run(
        &request,
        ctx.clone(),
        &run,
        &hard_policy,
        &mcp_server_command,
    )
    .await;
    match &result {
        Ok(resp) => {
            let _ = runlog::write_result(&run, &resp.text, resp.debug.as_ref());
        }
        Err(err) => {
            let _ = runlog::write_error(&run, err);
        }
    }
    result
}

#[derive(serde::Serialize, Clone)]
struct JobDonePayload {
    job_id: String,
    ok: bool,
    text: Option<String>,
    error: Option<String>,
    #[serde(default)]
    debug: Option<serde_json::Value>,
    run_dir: String,
}

#[tauri::command]
async fn brain_run_async(
    app: tauri::AppHandle,
    request: brain::BrainRequest,
) -> Result<String, String> {
    let ctx = interceptor::last_context_snapshot();
    let run = runlog::start_run(&app, &request.input, ctx.as_ref())?;
    let hard_policy = policy::HardPolicy::load(&app);
    let _ = runlog::write_hard_policy(&run, &hard_policy);
    let job_id = run.id.clone();
    let app_handle = app.clone();
    let request_clone = request.clone();
    let ctx_clone = ctx.clone();
    let policy_clone = hard_policy.clone();
    let mcp_server_command = tool_server::mcp_server_command();

    tauri::async_runtime::spawn(async move {
        let result = runtime::run(
            &request_clone,
            ctx_clone,
            &run,
            &policy_clone,
            &mcp_server_command,
        )
        .await;
        let (ok, text, error, debug) = match &result {
            Ok(resp) => {
                let _ = runlog::write_result(&run, &resp.text, resp.debug.as_ref());
                (true, Some(resp.text.clone()), None, resp.debug.clone())
            }
            Err(err) => {
                let _ = runlog::write_error(&run, err);
                (false, None, Some(err.clone()), None)
            }
        };

        let payload = JobDonePayload {
            job_id: run.id.clone(),
            ok,
            text,
            error,
            debug,
            run_dir: run.dir.to_string_lossy().to_string(),
        };
        let _ = app_handle.emit("job_done", payload);
    });

    Ok(job_id)
}

fn main() {
    load_dotenv();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            brain_run,
            brain_run_async
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            interceptor::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("fatal: Darling failed to start");
}
