use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn extract_output_text(raw: &serde_json::Value) -> Option<String> {
    // Some responses include a top-level `output_text`.
    if let Some(t) = raw.get("output_text").and_then(|v| v.as_str()) {
        let s = t.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }

    // Otherwise iterate `output[*].content[*]` and join all output_text parts.
    let mut out = String::new();
    let Some(arr) = raw.get("output").and_then(|v| v.as_array()) else {
        return None;
    };
    for item in arr {
        let Some(contents) = item.get("content").and_then(|v| v.as_array()) else {
            continue;
        };
        for c in contents {
            if c.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() { None } else { Some(out) }
}

pub async fn chat(
    client: &reqwest::Client,
    request: LlmChatRequest,
) -> Result<LlmChatResponse, LlmError> {
    let (base_url, api_key, headers) = match &request.provider {
        LlmProvider::OpenaiResponses {
            base_url,
            api_key,
            headers,
        } => (
            base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
            api_key
                .clone()
                .ok_or_else(|| LlmError::Message("[llm/openai_responses] Missing api_key".to_string()))?,
            headers.clone().unwrap_or_default(),
        ),
        _ => unreachable!("provider mismatch"),
    };

    let url = join_url(&base_url, "/responses");

    let input = request
        .messages
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "role": match m.role {
                    LlmRole::System => "system",
                    LlmRole::User => "user",
                    LlmRole::Assistant => "assistant",
                },
                "content": [
                    { "type": "input_text", "text": m.content }
                ]
            })
        })
        .collect::<Vec<_>>();

    let mut body = serde_json::json!({
        "model": request.model,
        "input": input,
        "max_output_tokens": request.max_tokens.unwrap_or(1024),
    });
    if let Some(t) = request.temperature {
        body["temperature"] = serde_json::json!(t);
    }
    if let Some(tp) = request.top_p {
        body["top_p"] = serde_json::json!(tp);
    }

    let mut req = client.post(url).json(&body).bearer_auth(api_key);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req.send().await?.error_for_status()?;
    let raw: serde_json::Value = resp.json().await?;
    let text = extract_output_text(&raw).unwrap_or_default();
    if text.is_empty() {
        return Err(LlmError::Message(format!(
            "[llm/openai_responses] Empty response text. Raw: {raw}"
        )));
    }

    Ok(LlmChatResponse {
        text,
        raw: Some(raw),
    })
}

