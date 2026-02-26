use std::{collections::HashMap, time::Duration};

use serde::{Deserialize, Serialize};

mod providers;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LlmProvider {
    /// OpenAI "Chat Completions" compatible providers.
    ///
    /// Works with OpenAI itself and most OpenAI-compatible vendors by customizing `base_url`.
    OpenaiCompat {
        /// Example: `https://api.openai.com/v1` or a vendor `.../v1`.
        base_url: Option<String>,
        api_key: Option<String>,
        /// Extra headers to add on every request.
        headers: Option<HashMap<String, String>>,
    },
    /// OpenAI Responses API.
    ///
    /// Note: this is NOT the same as OpenAI-compatible Chat Completions.
    OpenaiResponses {
        /// Example: `https://api.openai.com/v1`
        base_url: Option<String>,
        api_key: Option<String>,
        /// Extra headers to add on every request.
        headers: Option<HashMap<String, String>>,
    },
    /// Anthropic Messages API.
    Anthropic {
        /// Example: `https://api.anthropic.com`.
        base_url: Option<String>,
        api_key: Option<String>,
        /// Example: `2023-06-01`.
        version: Option<String>,
    },
    /// Ollama local server.
    Ollama {
        /// Example: `http://localhost:11434`.
        base_url: Option<String>,
    },
    /// Google Gemini "generateContent".
    Gemini {
        /// Example: `https://generativelanguage.googleapis.com`.
        base_url: Option<String>,
        api_key: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmChatRequest {
    pub provider: LlmProvider,
    pub model: String,
    pub messages: Vec<LlmMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmChatResponse {
    pub text: String,
    #[serde(default)]
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LlmError {
    pub fn user(self) -> String {
        match self {
            Self::Message(msg) => msg,
            other => other.to_string(),
        }
    }
}

pub async fn chat(request: LlmChatRequest) -> Result<LlmChatResponse, LlmError> {
    let timeout = Duration::from_millis(request.timeout_ms.unwrap_or(60_000));
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("Darling/0.1.0 (tauri)")
        .build()?;

    match &request.provider {
        LlmProvider::OpenaiCompat { .. } => providers::openai_compat::chat(&client, request).await,
        LlmProvider::OpenaiResponses { .. } => {
            providers::openai_responses::chat(&client, request).await
        }
        LlmProvider::Anthropic { .. } => providers::anthropic::chat(&client, request).await,
        LlmProvider::Ollama { .. } => providers::ollama::chat(&client, request).await,
        LlmProvider::Gemini { .. } => providers::gemini::chat(&client, request).await,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmEnvSettings {
    pub provider: LlmProvider,
    pub model: String,
}

/// Convenience for the UI: read a single "active" provider from env vars.
///
/// - `DARLING_LLM_KIND`: `openai_compat` | `anthropic` | `ollama` | `gemini`
/// - `DARLING_LLM_MODEL`: model name (required)
/// - `DARLING_LLM_BASE_URL`: optional base URL (provider-specific default)
/// - `DARLING_LLM_API_KEY`: optional API key (required for most cloud providers)
pub fn settings_from_env() -> Result<LlmEnvSettings, LlmError> {
    let kind = std::env::var("DARLING_LLM_KIND").unwrap_or_else(|_| "openai_compat".to_string());
    let model = std::env::var("DARLING_LLM_MODEL")
        .map_err(|_| LlmError::Message("[llm] Missing env: DARLING_LLM_MODEL".to_string()))?;
    let base_url = std::env::var("DARLING_LLM_BASE_URL").ok();
    let api_key = std::env::var("DARLING_LLM_API_KEY").ok();

    let provider = match kind.as_str() {
        "openai_compat" | "openai" | "openai-compatible" => LlmProvider::OpenaiCompat {
            base_url,
            api_key,
            headers: None,
        },
        "openai_responses" | "responses" => LlmProvider::OpenaiResponses {
            base_url,
            api_key,
            headers: None,
        },
        "anthropic" => LlmProvider::Anthropic {
            base_url,
            api_key,
            version: None,
        },
        "ollama" => LlmProvider::Ollama { base_url },
        "gemini" => LlmProvider::Gemini { base_url, api_key },
        other => {
            return Err(LlmError::Message(format!(
                "[llm] Unsupported DARLING_LLM_KIND: {other}"
            )))
        }
    };

    Ok(LlmEnvSettings { provider, model })
}

pub async fn prompt_from_env(prompt: String) -> Result<String, LlmError> {
    let settings = settings_from_env()?;
    let request = LlmChatRequest {
        provider: settings.provider,
        model: settings.model,
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: prompt,
        }],
        temperature: None,
        top_p: None,
        max_tokens: None,
        stop: None,
        timeout_ms: None,
    };

    let response = chat(request).await?;
    Ok(response.text)
}
