use crate::llm::{LlmChatRequest, LlmChatResponse, LlmError, LlmProvider, LlmRole};

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

pub async fn chat(client: &reqwest::Client, request: LlmChatRequest) -> Result<LlmChatResponse, LlmError> {
    let (base_url, api_key) = match &request.provider {
        LlmProvider::Gemini { base_url, api_key } => (
            base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string()),
            api_key
                .clone()
                .ok_or_else(|| LlmError::Message("[llm/gemini] Missing api_key".to_string()))?,
        ),
        _ => unreachable!("provider mismatch"),
    };

    // Gemini uses "contents" with role + parts. We'll map:
    // - system: prepend as first user content (simple + broadly compatible)
    let mut sys_prefix = String::new();
    let mut contents: Vec<serde_json::Value> = Vec::new();
    for m in request.messages {
        match m.role {
            LlmRole::System => {
                if sys_prefix.is_empty() {
                    sys_prefix = m.content;
                } else {
                    sys_prefix.push_str("\n\n");
                    sys_prefix.push_str(&m.content);
                }
            }
            LlmRole::User => {
                let text = if sys_prefix.is_empty() {
                    m.content
                } else {
                    let s = std::mem::take(&mut sys_prefix);
                    format!("{s}\n\n{}", m.content)
                };
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{ "text": text }]
                }));
            }
            LlmRole::Assistant => contents.push(serde_json::json!({
                "role": "model",
                "parts": [{ "text": m.content }]
            })),
        }
    }

    if !sys_prefix.is_empty() && contents.is_empty() {
        contents.push(serde_json::json!({
            "role": "user",
            "parts": [{ "text": sys_prefix }]
        }));
    }

    // Endpoint: /v1beta/models/{model}:generateContent?key=...
    let path = format!("/v1beta/models/{}:generateContent", request.model);
    let url = join_url(&base_url, &path);

    let mut body = serde_json::json!({
        "contents": contents,
    });
    if request.temperature.is_some() || request.top_p.is_some() || request.max_tokens.is_some() || request.stop.is_some() {
        let mut cfg = serde_json::Map::new();
        if let Some(t) = request.temperature {
            cfg.insert("temperature".to_string(), serde_json::json!(t));
        }
        if let Some(tp) = request.top_p {
            cfg.insert("topP".to_string(), serde_json::json!(tp));
        }
        if let Some(mt) = request.max_tokens {
            cfg.insert("maxOutputTokens".to_string(), serde_json::json!(mt));
        }
        if let Some(stop) = request.stop {
            cfg.insert("stopSequences".to_string(), serde_json::json!(stop));
        }
        body["generationConfig"] = serde_json::Value::Object(cfg);
    }

    let resp = client
        .post(url)
        .query(&[("key", api_key)])
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let raw: serde_json::Value = resp.json().await?;
    let mut out = String::new();
    if let Some(parts) = raw
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c0| c0.get("content"))
        .and_then(|ct| ct.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in parts {
            if let Some(txt) = part.get("text").and_then(|t| t.as_str()) {
                out.push_str(txt);
            }
        }
    }

    if out.is_empty() {
        return Err(LlmError::Message(format!(
            "[llm/gemini] Empty response text. Raw: {raw}"
        )));
    }

    Ok(LlmChatResponse {
        text: out,
        raw: Some(raw),
    })
}

