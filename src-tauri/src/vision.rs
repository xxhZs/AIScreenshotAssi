use std::{collections::HashMap, fs};

use base64::Engine;
use crate::interceptor::ContextSnapshot;

fn join_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    format!("{base}/{path}")
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn post_filter(text: String) -> String {
    // Simple noise reduction (language-agnostic):
    // - drop empty lines
    // - drop obviously UI-only lines
    // - de-duplicate consecutive duplicates
    // - drop common build/log boilerplate (unless it looks like an error block)
    let mut out = Vec::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }

        // Keep explicit error lines even if they look like logs.
        let lower_full = l.to_ascii_lowercase();
        let looks_error = lower_full.contains("error:")
            || lower_full.contains("exception")
            || lower_full.contains("panic")
            || lower_full.contains("traceback")
            || lower_full.contains("undefined symbols");

        if !looks_error {
            // Drop self-debug / meta content that commonly appears in our own screenshots.
            if l.contains("last_brain_debug")
                || l.contains("show_capsule_context")
                || l.contains("system_prompt_preview")
            {
                continue;
            }

            // Drop common build noise.
            if lower_full.starts_with("warning:")
                || lower_full.starts_with("note:")
                || lower_full.starts_with("finished `")
                || lower_full.starts_with("running `")
                || lower_full.contains("target/debug")
                || lower_full.contains("dev profile")
                || lower_full.contains("generated")
                || lower_full.contains("files changed")
                || lower_full.contains("src-tauri/")
            {
                continue;
            }

            // Drop very pathy lines (but keep if it looks like an error).
            let pathy = l.contains("/Users/") || l.contains("/var/folders/") || l.contains(".rs:");
            if pathy {
                continue;
            }
        }

        let lower = l.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "file" | "edit" | "view" | "help" | "window" | "search" | "navigate" | "terminal"
        ) {
            continue;
        }
        let s = l.to_string();
        if last.as_deref() == Some(&s) {
            continue;
        }
        last = Some(s.clone());
        out.push(s);
    }
    out.join("\n")
}

const LOCAL_VISION_OCR_JXA: &str = r#"
ObjC.import('Foundation');
ObjC.import('AppKit');
ObjC.import('Vision');

function run(argv) {
  if (!argv || argv.length < 1) return '';
  var path = argv[0];

  var url = $.NSURL.fileURLWithPath($(path));
  var image = $.NSImage.alloc.initWithContentsOfURL(url);
  if (!image) return '';

  var cg = image.CGImageForProposedRectContextHints(null, null, null);
  if (!cg) return '';

  var req = $.VNRecognizeTextRequest.alloc.init();
  req.recognitionLevel = $.VNRequestTextRecognitionLevelAccurate;
  req.usesLanguageCorrection = true;

  var handler = $.VNImageRequestHandler.alloc.initWithCGImageOptions(cg, $());
  var err = Ref();
  handler.performRequestsError($([req]), err);
  if (err[0]) return '';

  var results = req.results;
  if (!results) return '';

  var out = [];
  for (var i = 0; i < results.count; i++) {
    var obs = results.objectAtIndex(i);
    var cands = obs.topCandidates(1);
    if (cands && cands.count > 0) {
      out.push(ObjC.unwrap(cands.objectAtIndex(0).string));
    }
  }
  return out.join('\\n').trim();
}
"#;

fn local_vision_ocr(screenshot_path: &str) -> Result<Option<String>, String> {
    let output = std::process::Command::new("osascript")
        .args(["-l", "JavaScript", "-e", LOCAL_VISION_OCR_JXA, screenshot_path])
        .output()
        .map_err(|e| format!("[vision/local_ocr] spawn error: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            return Ok(None);
        }
        return Err(format!("[vision/local_ocr] {stderr}"));
    }

    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(s))
    }
}

/// Extract useful on-screen text from a screenshot using a dedicated vision-capable model.
///
/// This keeps the *main* LLM a black-box text model: the generator only receives a compact
/// "screen context" summary derived from the image.
pub async fn extract_screen_context_from_screenshot(
    screenshot_path: &str,
    ctx: Option<&ContextSnapshot>,
    user_goal: &str,
) -> Result<Option<String>, String> {
    // Toggle: keep it opt-in because it can be slow + privacy-sensitive.
    // If the flag is unset, auto-enable when the vision model + key are present
    // (helps avoid "configured but not used" confusion).
    let enabled = match std::env::var("DARLING_VISION_EXTRACT") {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => env_opt("DARLING_VISION_MODEL").is_some() && env_opt("DARLING_VISION_API_KEY").is_some(),
    };
    if !enabled {
        return Ok(None);
    }

    let kind = env_opt("DARLING_VISION_KIND").unwrap_or_else(|| "openai_responses".to_string());
    if kind == "local_ocr" {
        let text = local_vision_ocr(screenshot_path)?;
        return Ok(text.map(post_filter).filter(|t| !t.is_empty()));
    }
    if kind != "openai_responses" {
        return Err(format!(
            "[vision] Unsupported DARLING_VISION_KIND={kind} (supported: openai_responses, local_ocr)"
        ));
    }

    let api_key = env_opt("DARLING_VISION_API_KEY")
        .ok_or_else(|| "[vision] Missing env: DARLING_VISION_API_KEY".to_string())?;
    let model = env_opt("DARLING_VISION_MODEL")
        .ok_or_else(|| "[vision] Missing env: DARLING_VISION_MODEL".to_string())?;
    let base_url = env_opt("DARLING_VISION_BASE_URL").unwrap_or_else(|| "https://api.openai.com/v1".to_string());

    // Optional extra headers (JSON string) for proxies/gateways.
    // Example: DARLING_VISION_HEADERS='{"X-Title":"Darling"}'
    let headers: HashMap<String, String> = match env_opt("DARLING_VISION_HEADERS") {
        Some(v) => serde_json::from_str(&v).unwrap_or_default(),
        None => HashMap::new(),
    };

    let bytes = fs::read(screenshot_path)
        .map_err(|e| format!("[vision] Failed to read screenshot {screenshot_path}: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let data_url = format!("data:image/png;base64,{b64}");

    let mut hint = String::new();
    if let Some(c) = ctx {
        if let Some(app) = &c.app_name {
            hint.push_str(&format!("app_name: {app}\n"));
        }
        if let Some(title) = &c.window_title {
            hint.push_str(&format!("window_title: {title}\n"));
        }
    }

    let user_goal = user_goal.trim();

    // Prompt: produce a compact "image context" block that helps the main (text-only) LLM.
    // The user wants this to include an "intent / what I'm doing" feel, not raw OCR dumps.
    let prompt = format!(
        "You are a macOS screen understanding module.\n\
Your job is to convert a screenshot into a compact 'screen context' that helps another LLM respond.\n\
\n\
Output format:\n\
- Output ONLY plain text.\n\
- Start with a 1-line summary: \"SCREEN CONTEXT: <...>\".\n\
- Then provide 3 short sections with bullet points:\n\
  1) \"What I'm doing\" (your best guess from the screen)\n\
  2) \"Likely intent\" (what the user is probably trying to achieve)\n\
  3) \"Key evidence\" (ONLY the minimum essential on-screen text snippets / errors / code lines)\n\
\n\
Rules:\n\
- Be conservative: if unsure, say \"uncertain\" and give 1 clarifying question.\n\
- Ignore UI chrome: menus, toolbars, icons, sidebars, file trees, status bars, timestamps.\n\
- DO NOT include build output/tool logs unless the screen is clearly about an error/debugging workflow.\n\
- Remove noise and duplication; keep it under ~40 lines and under ~1200 characters.\n\
\n\
User's current instruction (may be short):\n\
{user_goal}\n\
\n\
Context hints (not from image):\n\
{hint}"
    );

    let url = join_url(&base_url, "/responses");
    let body = serde_json::json!({
        "model": model,
        "input": [{
            "role": "user",
            "content": [
                { "type": "input_text", "text": prompt },
                { "type": "input_image", "image_url": data_url }
            ]
        }],
        "max_output_tokens": 900
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(90_000))
        .user_agent("Darling/0.1.0 (vision-extract)")
        .build()
        .map_err(|e| format!("[vision] reqwest build error: {e}"))?;

    let mut req = client.post(url).json(&body).bearer_auth(api_key);
    for (k, v) in headers {
        req = req.header(k, v);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("[vision] request error: {e}"))?
        .error_for_status()
        .map_err(|e| format!("[vision] http status error: {e}"))?;

    let raw: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("[vision] json decode error: {e}"))?;

    let text = raw
        .get("output_text")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // Fallback to scanning output array.
            let mut out = String::new();
            let arr = raw.get("output")?.as_array()?;
            for item in arr {
                let contents = item.get("content")?.as_array()?;
                for c in contents {
                    if c.get("type")?.as_str()? == "output_text" {
                        if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                            out.push_str(t);
                        }
                    }
                }
            }
            let out = out.trim().to_string();
            if out.is_empty() { None } else { Some(out) }
        });

    let Some(text) = text else {
        return Ok(None);
    };
    let mut text = post_filter(text);
    if text.len() > 1200 {
        text = format!("{}…", &text[..1200]);
    }
    if text.is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}
