use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardPolicy {
    pub allow_network: bool,
    pub allow_exec: bool,
    pub allow_fs_read: bool,
    pub allow_fs_write: bool,
    pub allow_clipboard: bool,
    pub allow_screenshot: bool,
    pub allowed_roots: Vec<String>,
    pub blocked_roots: Vec<String>,
    pub max_steps: usize,
    pub max_tokens_per_call: u32,
    pub allowed_roles: Vec<String>,
    pub allowed_tools: Vec<String>,
}

fn is_path_allowed(policy: &HardPolicy, path: &Path) -> bool {
    let p = path.to_string_lossy().to_string();
    if policy.blocked_roots.iter().any(|b| p.starts_with(b)) {
        return false;
    }
    if policy.allowed_roots.is_empty() {
        return true;
    }
    policy.allowed_roots.iter().any(|a| p.starts_with(a))
}

pub fn check_path_allowed(policy: &HardPolicy, path: &Path) -> Result<(), String> {
    if is_path_allowed(policy, path) {
        Ok(())
    } else {
        Err(format!("path not allowed: {}", path.to_string_lossy()))
    }
}

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(&manifest_dir).to_path_buf()
}

impl HardPolicy {
    pub fn default_aggressive() -> Self {
        let root = repo_root().to_string_lossy().to_string();
        Self {
            allow_network: true,
            allow_exec: true,
            allow_fs_read: true,
            allow_fs_write: true,
            allow_clipboard: true,
            allow_screenshot: true,
            allowed_roots: vec![root],
            blocked_roots: vec!["/System".into(), "/Library".into(), "/private".into()],
            max_steps: 8,
            max_tokens_per_call: 3000,
            allowed_roles: vec![
                "generalist".into(),
                "outliner".into(),
                "drafter".into(),
                "polisher".into(),
                "critic".into(),
                "reviser".into(),
                "analyst".into(),
                "coder".into(),
            ],
            allowed_tools: vec!["read_file".into(), "list_dir".into(), "get_context".into()],
        }
    }

    pub fn load(app: &tauri::AppHandle) -> Self {
        let mut candidates = Vec::new();
        if let Ok(dir) = app.path().app_data_dir() {
            candidates.push(dir.join("hard_policy.json"));
        }
        candidates.push(repo_root().join("hard_policy.json"));

        for path in candidates {
            if let Ok(text) = fs::read_to_string(&path) {
                if let Ok(policy) = serde_json::from_str::<HardPolicy>(&text) {
                    return policy;
                }
            }
        }

        Self::default_aggressive()
    }

    pub fn summary(&self) -> String {
        format!(
            "constraints:\n- max_steps: {}\n- max_tokens_per_call: {}\n- allowed_roles: {}\n- allowed_tools: {}\n",
            self.max_steps,
            self.max_tokens_per_call,
            self.allowed_roles.join(", "),
            self.allowed_tools.join(", ")
        )
    }
}
