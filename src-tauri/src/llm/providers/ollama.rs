use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

pub async fn chat(client: &reqwest::Client, request: LlmChatRequest) -> Result<LlmChatResponse, LlmError> {
    let base_url = match &request.provider {
        LlmProvider::Ollama { base_url } => base_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".to_string()),
        _ => unreachable!("provider mismatch"),
    };

    let url = join_url(&base_url, "/api/chat");
    let body = serde_json::json!({
        "model": request.model,
        "messages": request.messages.into_iter().map(|m| {
            serde_json::json!({
                "role": match m.role {
                    LlmRole::System => "system",
                    LlmRole::User => "user",
                    LlmRole::Assistant => "assistant",
                },
                "content": m.content,
            })
        }).collect::<Vec<_>>(),
        "stream": false,
        "options": {
            "temperature": request.temperature,
            "top_p": request.top_p,
        }
    });

    let resp = client.post(url).json(&body).send().await?.error_for_status()?;
    let raw: serde_json::Value = resp.json().await?;
    let text = raw
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        return Err(LlmError::Message(format!(
            "[llm/ollama] Empty response text. Raw: {raw}"
        )));
    }

    Ok(LlmChatResponse {
        text,
        raw: Some(raw),
    })
}

