use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct HardPolicy {
    #[serde(default = "default_true")]
    allow_fs_read: bool,
    #[serde(default)]
    allowed_roots: Vec<String>,
    #[serde(default)]
    blocked_roots: Vec<String>,
    #[serde(default = "default_allowed_tools")]
    allowed_tools: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_allowed_tools() -> Vec<String> {
    vec!["read_file".to_string(), "list_dir".to_string()]
}

impl Default for HardPolicy {
    fn default() -> Self {
        let root = repo_root().to_string_lossy().to_string();
        Self {
            allow_fs_read: true,
            allowed_roots: vec![root],
            blocked_roots: vec![
                "/System".to_string(),
                "/Library".to_string(),
                "/private".to_string(),
            ],
            allowed_tools: default_allowed_tools(),
        }
    }
}

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(&manifest_dir).to_path_buf()
}

fn load_policy() -> HardPolicy {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("DARLING_HARD_POLICY_PATH") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(repo_root().join("hard_policy.json"));
    for path in candidates {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(policy) = serde_json::from_str::<HardPolicy>(&text) {
                return policy;
            }
        }
    }
    HardPolicy::default()
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

fn read_message(reader: &mut BufReader<std::io::Stdin>) -> io::Result<Option<serde_json::Value>> {
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse::<usize>().unwrap_or(0);
        }
    }

    if content_length == 0 {
        return Ok(None);
    }

    let mut buf = vec![0u8; content_length];
    reader.read_exact(&mut buf)?;
    let value = serde_json::from_slice::<serde_json::Value>(&buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(Some(value))
}

fn write_message(stdout: &mut std::io::StdoutLock<'_>, value: &serde_json::Value) -> io::Result<()> {
    let body =
        serde_json::to_vec(value).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    write!(stdout, "Content-Length: {}\r\n\r\n", body.len())?;
    stdout.write_all(&body)?;
    stdout.flush()?;
    Ok(())
}

fn tools_list(policy: &HardPolicy) -> serde_json::Value {
    let tools = policy
        .allowed_tools
        .iter()
        .map(|name| match name.as_str() {
            "read_file" => serde_json::json!({
                "name": "read_file",
                "description": "Read a file from disk",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }),
            "list_dir" => serde_json::json!({
                "name": "list_dir",
                "description": "List a directory",
                "inputSchema": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } }
                }
            }),
            other => serde_json::json!({
                "name": other,
                "description": "Unknown tool",
                "inputSchema": { "type": "object", "properties": {} }
            }),
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "tools": tools })
}

fn tool_read_file(args: &serde_json::Value, policy: &HardPolicy) -> Result<String, String> {
    if !policy.allow_fs_read {
        return Err("read_file not permitted by policy".to_string());
    }
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing path".to_string())?;
    let p = PathBuf::from(path);
    if !is_path_allowed(policy, &p) {
        return Err(format!("path not allowed: {}", p.to_string_lossy()));
    }
    fs::read_to_string(&p).map_err(|e| e.to_string())
}

fn tool_list_dir(args: &serde_json::Value, policy: &HardPolicy) -> Result<String, String> {
    if !policy.allow_fs_read {
        return Err("list_dir not permitted by policy".to_string());
    }
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let p = PathBuf::from(path);
    if !is_path_allowed(policy, &p) {
        return Err(format!("path not allowed: {}", p.to_string_lossy()));
    }
    let mut out = Vec::new();
    for item in fs::read_dir(p).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        out.push(item.file_name().to_string_lossy().to_string());
    }
    out.sort();
    Ok(out.join("\n"))
}

fn tools_call(name: &str, args: &serde_json::Value, policy: &HardPolicy) -> Result<serde_json::Value, String> {
    if !policy.allowed_tools.iter().any(|t| t == name) {
        return Err(format!("tool not allowed: {name}"));
    }
    let text = match name {
        "read_file" => tool_read_file(args, policy)?,
        "list_dir" => tool_list_dir(args, policy)?,
        other => return Err(format!("unknown tool: {other}")),
    };
    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

fn main() {
    let policy = load_policy();
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin);
    let stdout = io::stdout();
    let mut writer = stdout.lock();

    loop {
        let Some(message) = read_message(&mut reader).unwrap_or(None) else {
            break;
        };
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let params = message
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Notification without id: no response required.
        if id.is_none() {
            continue;
        }
        let id = id.unwrap_or(serde_json::Value::Null);

        let response = match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "serverInfo": { "name": "darling-mcp", "version": "0.1.0" },
                    "capabilities": { "tools": {} }
                }
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": tools_list(&policy)
            }),
            "tools/call" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                match tools_call(name, &args, &policy) {
                    Ok(result) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result
                    }),
                    Err(err) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32000, "message": err }
                    }),
                }
            }
            _ => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            }),
        };

        if write_message(&mut writer, &response).is_err() {
            break;
        }
    }
}
