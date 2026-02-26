use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};
use base64::Engine;
use std::fs;

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn guess_mime(path: &str) -> &'static str {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".jpg") || p.ends_with(".jpeg") {
        "image/jpeg"
    } else if p.ends_with(".webp") {
        "image/webp"
    } else {
        "image/png"
    }
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

    let mut messages: Vec<serde_json::Value> = Vec::new();
    for m in request.messages {
        // Multimodal (best-effort): for user messages with images, send content parts.
        // Providers that don't support this may error; in that case users should switch to `openai_responses`.
        if matches!(m.role, LlmRole::User) {
            if let Some(paths) = m.image_paths {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                parts.push(serde_json::json!({ "type": "text", "text": m.content }));
                let total = paths.len();
                for (i, p) in paths.iter().enumerate() {
                    if total > 1 {
                        let label = if i == 0 { "trigger view" } else { "after scroll" };
                        parts.push(serde_json::json!({
                            "type": "text",
                            "text": format!("Screenshot {}/{} ({label})", i + 1, total)
                        }));
                    }
                    let bytes = fs::read(p).map_err(|e| {
                        LlmError::Message(format!(
                            "[llm/openai_compat] Failed to read image {p}: {e}"
                        ))
                    })?;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                    let mime = guess_mime(p);
                    parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{mime};base64,{b64}") }
                    }));
                }
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": parts
                }));
                continue;
            }
        }

        messages.push(serde_json::json!({
            "role": match m.role {
                LlmRole::System => "system",
                LlmRole::User => "user",
                LlmRole::Assistant => "assistant",
            },
            "content": m.content,
        }));
    }

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
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
