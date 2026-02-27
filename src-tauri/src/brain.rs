use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainRequest {
    pub input: String,
    #[serde(default)]
    pub debug: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainResponse {
    pub text: String,
    #[serde(default)]
    pub debug: Option<serde_json::Value>,
}
