use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

pub async fn chat(client: &reqwest::Client, request: LlmChatRequest) -> Result<LlmChatResponse, LlmError> {
    let (base_url, api_key, headers) = match &request.provider {
        LlmProvider::OpenaiCompat {
            base_url,
            api_key,
            headers,
        } => (
            base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            api_key.clone(),
            headers.clone().unwrap_or_default(),
        ),
        _ => unreachable!("provider mismatch"),
    };

    let url = join_url(&base_url, "/chat/completions");

    let mut body = serde_json::json!({
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
    });

    if let Some(t) = request.temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(tp) = request.top_p {
        body["top_p"] = serde_json::json!(tp);
    }
    if let Some(mt) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(mt);
    }
    if let Some(stop) = request.stop {
        body["stop"] = serde_json::json!(stop);
    }

    let mut req = client.post(url).json(&body);
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req.send().await?.error_for_status()?;
    let raw: serde_json::Value = resp.json().await?;
    let text = raw
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    if text.is_empty() {
        return Err(LlmError::Message(format!(
            "[llm/openai_compat] Empty response text. Raw: {raw}"
        )));
    }

    Ok(LlmChatResponse {
        text,
        raw: Some(raw),
    })
}

