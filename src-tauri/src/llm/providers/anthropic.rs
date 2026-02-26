use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

pub async fn chat(client: &reqwest::Client, request: LlmChatRequest) -> Result<LlmChatResponse, LlmError> {
    let (base_url, api_key, version) = match &request.provider {
        LlmProvider::Anthropic {
            base_url,
            api_key,
            version,
        } => (
            base_url
                .clone()
                .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
            api_key
                .clone()
                .ok_or_else(|| LlmError::Message("[llm/anthropic] Missing api_key".to_string()))?,
            version.clone().unwrap_or_else(|| "2023-06-01".to_string()),
        ),
        _ => unreachable!("provider mismatch"),
    };

    // Anthropic has a dedicated top-level `system` field (string), and the rest are messages.
    let mut system: Option<String> = None;
    let mut messages = Vec::new();
    for m in request.messages {
        match m.role {
            LlmRole::System => {
                if system.is_none() {
                    system = Some(m.content);
                } else {
                    let existing = system.take().unwrap_or_default();
                    system = Some(format!("{existing}\n\n{}", m.content));
                }
            }
            LlmRole::User => messages.push(serde_json::json!({ "role": "user", "content": m.content })),
            LlmRole::Assistant => {
                messages.push(serde_json::json!({ "role": "assistant", "content": m.content }))
            }
        }
    }

    let url = join_url(&base_url, "/v1/messages");
    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens.unwrap_or(1024),
    });
    if let Some(sys) = system {
        body["system"] = serde_json::json!(sys);
    }
    if let Some(t) = request.temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(stop) = request.stop {
        body["stop_sequences"] = serde_json::json!(stop);
    }

    let resp = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", version)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let raw: serde_json::Value = resp.json().await?;
    let mut out = String::new();
    if let Some(arr) = raw.get("content").and_then(|c| c.as_array()) {
        for part in arr {
            if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                    out.push_str(txt);
                }
            }
        }
    }

    if out.is_empty() {
        return Err(LlmError::Message(format!(
            "[llm/anthropic] Empty response text. Raw: {raw}"
        )));
    }

    Ok(LlmChatResponse {
        text: out,
        raw: Some(raw),
    })
}

