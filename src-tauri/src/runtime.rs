use serde::{Deserialize, Serialize};

use crate::brain::{BrainRequest, BrainResponse};
use crate::interceptor::ContextSnapshot;
use crate::policy::HardPolicy;
use crate::runlog::RunHandle;

#[derive(Debug, Serialize)]
struct RuntimeRequest<'a> {
    input: &'a str,
    #[serde(default)]
    debug: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<ContextSnapshot>,
    run_id: &'a str,
    mcp_server_command: &'a str,
    hard_policy: &'a HardPolicy,
    run_dir: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeResponse {
    text: String,
    #[serde(default)]
    debug: Option<serde_json::Value>,
}

fn runtime_url() -> String {
    std::env::var("DARLING_RUNTIME_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:3999/run".to_string())
}

pub async fn run(
    request: &BrainRequest,
    context: Option<ContextSnapshot>,
    run: &RunHandle,
    hard_policy: &HardPolicy,
    mcp_server_command: &str,
) -> Result<BrainResponse, String> {
    let url = runtime_url();
    let payload = RuntimeRequest {
        input: &request.input,
        debug: request.debug,
        context,
        run_id: &run.id,
        mcp_server_command,
        hard_policy,
        run_dir: run.dir.to_string_lossy().to_string(),
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("runtime error: HTTP {}", resp.status()));
    }

    let body: RuntimeResponse = resp.json().await.map_err(|e| e.to_string())?;
    Ok(BrainResponse {
        text: body.text,
        debug: body.debug,
    })
}
